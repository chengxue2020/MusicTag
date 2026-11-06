// MusicTag — 发行年份分级兜底查询（FR-8 #17/#18/#19，design.md §10.3）。
//
// **不进常规五源并发链**：`search_metadata_fallback` 是独立 command，只有用户在
// 「年份 / 元数据」区手动点「补全年份」才发起；前端已确保本次常规搜索 `all_failed=false`
// （离线 / 全源超时不触发）。不落盘缓存、不自动写盘、候选仅内存流转。
//
// 分级、命中即停：
// - 一级：同平台备用端点（QQ 联想 `smartbox_new.fcg` → 单曲详情 `album.time_public`；
//   iTunes JP / KR 店面 `releaseDate`）——域名已在既有披露清单内；
// - 一级无可信年份才跑二级：Deezer `album.release_date`、MusicBrainz
//   `first-release-date`（官方要求 1 req/s 限流 + 描述性 User-Agent）。
// 各级内部并发、单源 6s 超时降级为空，单源失败不影响其他源。
//
// 沿用批量年份脚本的质量口径：
// - `publishTime = 0` 之类缺失值绝不换算成 1970（`dates` 模块统一守卫）；
// - 归一化必须**宽口径 Unicode**，否则俄文 / 日文假名 / 韩文歌名会被整段清空；
// - 歌名零关联、歌手明显不符一律丢弃（宁缺毋滥），年份须在 1900 ~ 次年之间；
// - 版本后缀（DJ版 / Live / Remix …）对称清洗，避免翻唱/重制版误配原曲年份。

use crate::model::MetadataCandidate;
use crate::service::searcher::{dates, itunes, join_query_terms, norm, MusicSource};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::task::JoinSet;

/// 单源请求 / 单级整体超时（FR-8 #19：各级内部并发、各自 6s）。
const LEVEL_TIMEOUT: Duration = Duration::from_secs(6);
/// 返回候选上限（PRD FR-8 #19：按匹配度降序最多 8 条）。
const MAX_RESULTS: usize = 8;
/// MusicBrainz 官方限流间隔（1 req/s）。
const MB_INTERVAL: Duration = Duration::from_secs(1);
/// MusicBrainz 要求描述性 User-Agent（官方 Rate Limiting 条款）。
const MB_USER_AGENT: &str =
    "MusicTag/0.2.4 ( https://github.com/zsxink/MusicTag ; metadata year lookup )";

/// 版本后缀清洗词表（查询串与候选标题同样处理，对称清洗不引入偏差）。
const VERSION_TAGS: &[&str] = &[
    "dj版", "烟嗓版", "0.8x", "1.2x", "live", "remix", "cover", "翻自", "explicit", "bootleg",
    "伴奏", "纯音乐", "dj",
];

/// 兜底命中的原始记录（未打分、未去重）。`pub`：供外置集成测试构造打分用例。
#[derive(Debug, Clone)]
pub struct RawHit {
    pub year: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub source: &'static str,
}

/// 宽口径归一化：先复用既有 `norm()`（繁→简 + 全角半角 + 小写），再只保留 Unicode
/// 字母数字。`char::is_alphanumeric()` 按 Unicode 判定——批量脚本曾因只保留
/// `[0-9a-z\u4e00-\u9fff]` 把俄文 / 日文假名 / 韩文整段清空，相似度恒为 0。
pub fn wide_norm(s: &str) -> String {
    norm(s)
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// 版本后缀清洗：先去括号内容，再（对查询与候选对称地）剥掉已知版本标记。
pub fn clean_title(s: &str) -> String {
    let mut depth = 0_i32;
    let mut no_brackets = String::new();
    for c in s.chars() {
        match c {
            '(' | '（' | '[' | '【' | '《' => depth += 1,
            ')' | '）' | ']' | '】' | '》' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ if depth == 0 => no_brackets.push(c),
            _ => {}
        }
    }
    let mut t = no_brackets.to_lowercase();
    for &tag in VERSION_TAGS {
        t = t.replace(tag, "");
    }
    t
}

/// 最长公共子序列比率（0.0 ~ 1.0），短文本上开销可忽略。
pub fn similarity(a: &str, b: &str) -> f32 {
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    if ac.is_empty() || bc.is_empty() {
        return 0.0;
    }
    let mut prev = vec![0_usize; bc.len() + 1];
    let mut cur = vec![0_usize; bc.len() + 1];
    for x in &ac {
        for (j, y) in bc.iter().enumerate() {
            cur[j + 1] = if x == y {
                prev[j] + 1
            } else {
                prev[j + 1].max(cur[j])
            };
        }
        std::mem::swap(&mut prev, &mut cur);
        cur.fill(0);
    }
    (2.0 * prev[bc.len()] as f32) / (ac.len() + bc.len()) as f32
}

/// 打分 / 过滤 / 去重 / 排序：把原始命中收敛成可展示候选。
///
/// - 歌名归一化后空串（本地无歌名）或相似度 < 0.60 → 丢弃（宁缺毋滥）；
/// - 歌手不匹配 → 扣 0.25，完全匹配 → 加 0.2；总分 < 0.55 丢弃；
/// - 年份再校验一次 `dates`（1900 ~ 次年），脏数据不进候选；
/// - 按 (年份, 归一化歌名) 去重，分数降序最多 `MAX_RESULTS` 条。
pub fn finalize(query_title: &str, query_artist: &str, hits: Vec<RawHit>) -> Vec<MetadataCandidate> {
    let qt = wide_norm(&clean_title(query_title));
    let qa = wide_norm(query_artist);
    if qt.is_empty() {
        return Vec::new();
    }
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut out: Vec<MetadataCandidate> = Vec::new();
    for h in hits {
        if dates::year_from_date_str(&h.year).is_none() {
            continue;
        }
        let ct = wide_norm(&clean_title(&h.title));
        if ct.is_empty() {
            continue;
        }
        let title_score = similarity(&qt, &ct);
        if title_score < 0.60 {
            continue;
        }
        let mut score = title_score;
        if !qa.is_empty() {
            let ca = wide_norm(&h.artist);
            if ca.is_empty() || similarity(&qa, &ca) < 0.34 {
                score -= 0.25; // 歌手不匹配扣分
            } else {
                score += 0.2;
            }
        }
        if score < 0.55 {
            continue;
        }
        if !seen.insert((h.year.clone(), ct)) {
            continue;
        }
        out.push(MetadataCandidate {
            year: h.year,
            title: h.title,
            artist: h.artist,
            album: h.album,
            source: h.source.to_string(),
            score,
        });
    }
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(MAX_RESULTS);
    out
}

/// 通用 GET JSON（单源 6s 超时；失败 / 非 2xx / 解析失败 → `Err`，由调用方降级为空）。
async fn get_json(
    client: &reqwest::Client,
    url: reqwest::Url,
    referer: Option<&str>,
    user_agent: Option<&str>,
) -> Result<serde_json::Value, String> {
    let mut req = client.get(url).timeout(LEVEL_TIMEOUT);
    if let Some(r) = referer {
        req = req.header("Referer", r);
    }
    if let Some(u) = user_agent {
        req = req.header("User-Agent", u);
    }
    let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("响应解析失败: {e}"))
}

fn text(v: &serde_json::Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

/// 一级：QQ 联想 → 单曲详情（`album.time_public`）。
async fn qq_detail_year(client: &reqwest::Client, mid: &str) -> Option<String> {
    let url = reqwest::Url::parse_with_params(
        "https://c.y.qq.com/v8/fcg-bin/fcg_play_single_song.fcg",
        &[("songmid", mid), ("format", "json")],
    )
    .ok()?;
    let json = get_json(client, url, Some("https://y.qq.com/"), None)
        .await
        .ok()?;
    let item = json["data"].as_array()?.first()?;
    let tp = item["album"]["time_public"]
        .as_str()
        .or_else(|| item["time_public"].as_str())?;
    dates::year_from_date_str(tp)
}

async fn qq_smartbox_hits(client: &reqwest::Client, kw: &str) -> Vec<RawHit> {
    let url = match reqwest::Url::parse_with_params(
        "https://c.y.qq.com/splcloud/fcgi-bin/smartbox_new.fcg",
        &[("is_xml", "0"), ("key", kw)],
    ) {
        Ok(u) => u,
        Err(_) => return Vec::new(),
    };
    let json = match get_json(client, url, Some("https://y.qq.com/"), None).await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if json["code"].as_i64().unwrap_or(0) != 0 {
        return Vec::new();
    }
    let items = match json["data"]["song"]["itemlist"].as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    // 只取前 4 条联想结果，详情请求并发（避免 4×6s 串行拖垮整级）。
    let mut set: JoinSet<RawHit> = JoinSet::new();
    for it in items.iter().take(4) {
        let mid = text(it, "mid");
        if mid.is_empty() {
            continue;
        }
        let name = text(it, "name");
        let singer = text(it, "singer");
        let client = client.clone();
        set.spawn(async move {
            let year = qq_detail_year(&client, &mid).await.unwrap_or_default();
            RawHit {
                year,
                title: name,
                artist: singer,
                album: String::new(),
                source: "QQ 单曲详情",
            }
        });
    }
    let mut out = Vec::new();
    while let Some(Ok(hit)) = set.join_next().await {
        if !hit.year.is_empty() {
            out.push(hit);
        }
    }
    out
}

/// 一级：复用既有 `Itunes` 源，仅把店面换成 JP / KR（同域名、不同店面）。
async fn itunes_storefront_hits(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    album: &str,
    storefront: &str,
    label: &'static str,
) -> Vec<RawHit> {
    let src = itunes::Itunes {
        search_url: "https://itunes.apple.com/search".to_string(),
        storefronts: vec![storefront.to_string()],
    };
    let list = match src.search(client, title, artist, album).await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    list.into_iter()
        .filter_map(|cand| {
            let year = cand.year.filter(|y| dates::year_from_date_str(y).is_some())?;
            Some(RawHit {
                year,
                title: cand.title,
                artist: cand.artist,
                album: cand.album,
                source: label,
            })
        })
        .collect()
}

async fn run_level1(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    album: &str,
    kw: &str,
) -> Vec<RawHit> {
    let mut set: JoinSet<Vec<RawHit>> = JoinSet::new();
    {
        let (c, k) = (client.clone(), kw.to_string());
        set.spawn(async move {
            match tokio::time::timeout(LEVEL_TIMEOUT, qq_smartbox_hits(&c, &k)).await {
                Ok(v) => v,
                Err(_) => Vec::new(),
            }
        });
    }
    for (storefront, label) in [("JP", "iTunes JP"), ("KR", "iTunes KR")] {
        let c = client.clone();
        let (t, a, al) = (title.to_string(), artist.to_string(), album.to_string());
        set.spawn(async move {
            match tokio::time::timeout(
                LEVEL_TIMEOUT,
                itunes_storefront_hits(&c, &t, &a, &al, storefront, label),
            )
            .await
            {
                Ok(v) => v,
                Err(_) => Vec::new(),
            }
        });
    }
    let mut out = Vec::new();
    while let Some(Ok(mut v)) = set.join_next().await {
        out.append(&mut v);
    }
    out
}

/// 二级：Deezer 搜索（`data[].album.release_date`）。
async fn deezer_hits(client: &reqwest::Client, kw: &str) -> Vec<RawHit> {
    let url = match reqwest::Url::parse_with_params(
        "https://api.deezer.com/search",
        &[("q", kw), ("limit", "20")],
    ) {
        Ok(u) => u,
        Err(_) => return Vec::new(),
    };
    let json = match get_json(client, url, None, None).await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let arr = match json["data"].as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|x| {
            let year = dates::year_from_date_str(
                x["album"]["release_date"].as_str().unwrap_or(""),
            )?;
            Some(RawHit {
                year,
                title: text(x, "title"),
                artist: text(&x["artist"], "name"),
                album: text(&x["album"], "title"),
                source: "Deezer",
            })
        })
        .collect()
}

/// MusicBrainz 1 req/s 串行闸门（进程级；只记录上次放行时刻，不跨 await 持锁）。
static MB_LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();

async fn wait_musicbrainz_slot() {
    let lock = MB_LAST.get_or_init(|| Mutex::new(None));
    let wait = {
        let mut guard = match lock.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let now = Instant::now();
        // `guard` 记的是「本次放行时刻」，可能略晚于 now（上一次的等待窗口）。
        // `duration_since` 在 earlier > now 时会 panic，故用饱和版本：重叠调用按满 1s 等待，保守但不炸。
        let wait = match *guard {
            Some(prev) => MB_INTERVAL.saturating_sub(now.saturating_duration_since(prev)),
            None => Duration::ZERO,
        };
        *guard = Some(now + wait);
        wait
    };
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
}

/// 二级：MusicBrainz recording 搜索（`first-release-date` / `releases[].date`）。
async fn musicbrainz_hits(client: &reqwest::Client, kw: &str) -> Vec<RawHit> {
    wait_musicbrainz_slot().await;
    let url = match reqwest::Url::parse_with_params(
        "https://musicbrainz.org/ws/2/recording/",
        &[("query", kw), ("fmt", "json"), ("limit", "8")],
    ) {
        Ok(u) => u,
        Err(_) => return Vec::new(),
    };
    let json = match get_json(client, url, None, Some(MB_USER_AGENT)).await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let arr = match json["recordings"].as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for r in arr {
        let mut year = dates::year_from_date_str(r["first-release-date"].as_str().unwrap_or(""));
        if year.is_none() {
            if let Some(rels) = r["releases"].as_array() {
                for rel in rels {
                    if let Some(y) = dates::year_from_date_str(rel["date"].as_str().unwrap_or("")) {
                        year = Some(y);
                        break;
                    }
                }
            }
        }
        let Some(year) = year else { continue };
        let artist = r["artist-credit"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x["name"].as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        let album = r["releases"]
            .as_array()
            .and_then(|rels| rels.first())
            .and_then(|rel| rel["title"].as_str())
            .unwrap_or_default()
            .to_string();
        out.push(RawHit {
            year,
            title: text(r, "title"),
            artist,
            album,
            source: "MusicBrainz",
        });
    }
    out
}

async fn run_level2(client: &reqwest::Client, kw: &str) -> Vec<RawHit> {
    let mut set: JoinSet<Vec<RawHit>> = JoinSet::new();
    {
        let (c, k) = (client.clone(), kw.to_string());
        set.spawn(async move {
            match tokio::time::timeout(LEVEL_TIMEOUT, deezer_hits(&c, &k)).await {
                Ok(v) => v,
                Err(_) => Vec::new(),
            }
        });
    }
    {
        let (c, k) = (client.clone(), kw.to_string());
        set.spawn(async move {
            // 限流闸门可能等待 1s，故该任务预算 = 6s 请求 + 1s 限流窗口。
            match tokio::time::timeout(LEVEL_TIMEOUT + MB_INTERVAL, musicbrainz_hits(&c, &k)).await {
                Ok(v) => v,
                Err(_) => Vec::new(),
            }
        });
    }
    let mut out = Vec::new();
    while let Some(Ok(mut v)) = set.join_next().await {
        out.append(&mut v);
    }
    out
}

/// 分级兜底入口（`search_metadata_fallback` command 的业务实现）。
///
/// 一级有可信年份即返回（命中即停，不再打二级）；一级全空才跑二级。
/// 任何一级失败都降级为空列表，不抛错、不标会话离线（手动兜底不影响常规搜索状态）。
pub async fn search_metadata_fallback(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    album: &str,
) -> Vec<MetadataCandidate> {
    let kw = join_query_terms(title, artist, album);
    if title.trim().is_empty() || kw.trim().is_empty() {
        return Vec::new();
    }
    let level1 = finalize(title, artist, run_level1(client, title, artist, album, &kw).await);
    if !level1.is_empty() {
        return level1;
    }
    finalize(title, artist, run_level2(client, &kw).await)
}
