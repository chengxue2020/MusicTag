// MusicTag — 独立在线歌词服务（online-lyrics-sources）。
//
// 两个只提供歌词的源：千千 Plus（`lyrics.qianqian.plus`）与 Rangotec
// （`tools.rangotec.com`）。它们**没有**标题以外的完整元信息（无封面 / 年份 / 流派 /
// 音轨），故按用户决定做成独立的「在线搜词」入口：
// - 不注册为 `MusicSource`，不进 `searcher::search_song` 五源并发链；
// - 不改 `MusicSourceId` / `source_stats` / `all_failed` / C2 换源链；
// - 新增 `search_lyrics` / `fetch_online_lyric` 两个 command，前端歌词面板独立状态机。
//
// 分层（design.md §10）：纯业务层，函数接 `&reqwest::Client`，可脱离 Tauri 单测；
// 命令薄壳在 `commands/search.rs`。生产 `src/` 零 `#[cfg(test)]`，测试外置 `tests/`。
//
// 协议要点（2026-11 实测）：
// - 千千 Plus 单端点双 action：
//   搜索 `?sh&Title=<...>&Artist=<...>` → XML `<result><lrc id artist title/></result>`；
//   取词 `?dl&Id=<id>` → 纯文本 LRC；响应头 `X-Lyric-Encoding: utf-8 | gbk`。
//   中文参数常返回 `charset=UTF-8`，纯 ASCII 参数可能返回 `charset=GBK` /
//   XML 声明 `GB2312`，LRC 正文也混用 UTF-8 与 GBK —— 必须按响应头/声明解码。
//   无结果时仍会返回一堆不相关热门候选，需由本地打分过滤（上游空结果语义不可用）。
// - Rangotec 公开 JSON API `GET /api/anon/lrc?title=&artist=&od=asc`：
//   UTF-8 JSON `{code,msg,data:[{id,artist,title,album,lrc,trc,...}]}`，最多 50 条，
//   `lrc` 全文随响应返回（这里仍按惰性拉取约定不随 IPC 带出，点选后按 id 取词）。
//   **HTTP 200 但 `code=400` 是无结果/错误**，必须先判业务码。
//   `od=asc` 取老数据（标准 `[mm:ss.xx]`）；`od=desc`/默认优先返回最新 UGC，
//   多为增强型双时间戳 `[00:00:820,00:04:100]` 且夹带版权/翻唱广告行 —— 默认只用 asc。
//   取词走老协议兼容端点 `GET /qianqian?dl?Id=<id>&Code=abc`（实测稳定返回同一份 LRC）。
//   单 IP 1 秒 100 次、凌晨 1–5 点可能关服务、对特殊字符过滤严格 → 查询词先做清洗。
//
// 清洗规则（用户决定）：去掉千千 Plus 的署名 / 版权声明；Rangotec 的版权警示与
// 翻唱广告行一并过滤；增强型双时间戳转标准 `[mm:ss.xx]`；去空元信息标签行。

use crate::model::{LyricCandidate, LyricsProvider};
use crate::service::searcher;
use std::collections::HashSet;
use std::time::Duration;

/// 单个请求超时（与主搜索单源 6s 同口径）。
const TIMEOUT: Duration = Duration::from_secs(6);
/// 每个 provider 最多保留的候选数（两源合计 ≤ 2×8）。
pub const PER_PROVIDER_TOP: usize = 8;

const QIANQIAN_BASE: &str = "https://lyrics.qianqian.plus/api/search/";
const RANGOTEC_SEARCH: &str = "https://tools.rangotec.com/api/anon/lrc";
const RANGOTEC_LYRIC: &str = "https://tools.rangotec.com/qianqian";

/// 无内容的 LRC 元信息标签行（Rangotec 老协议固定返回这四行空标签）。
const EMPTY_META_TAGS: [&str; 4] = ["[ti:]", "[ar:]", "[al:]", "[by:]"];

/// 署名 / 版权声明 / 广告行标记（小写比较，中文条目本身无大小写）。
const WATERMARK_MARKERS: [&str; 8] = [
    "qianqian.plus",
    "本歌词由",
    "来源于网络",
    "版权归原作者",
    "未经著作权人书面许可",
    "本歌曲翻唱由",
    "提供生产能力",
    "vemus",
];

/// 搜索 / 取词前的查询词清洗：去首尾空白、控制字符与容易触发上游严格过滤的符号。
/// 保留中英文、数字、空格与常见歌名标点（`-` `(` `)` `.` `,` `/` `&` `:` `+`）。
pub fn sanitize_query(s: &str) -> String {
    const BLOCKED: &str = "[]{}\\|;^`'\"%#?<>";
    s.trim()
        .chars()
        .filter(|c| !c.is_control() && !BLOCKED.contains(*c))
        .collect::<String>()
        .trim()
        .to_string()
}

/// RFC 3986 风格的 UTF-8 百分号编码（只保留 unreserved 字符）。
/// 千千 Plus 的 action 是无值的裸参数（`?sh&Title=...`），用 `Url::parse_with_params`
/// 会退化成 `sh=`，服务端不认，故手工拼查询串并逐字节编码参数值。
pub fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 从 `Content-Type` 中取 `charset=` 标签。
fn charset_label(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    let pos = lower.find("charset=")?;
    let tail = &content_type[pos + "charset=".len()..];
    let label = tail
        .split(';')
        .next()?
        .trim()
        .trim_matches(|c| c == '"' || c == '\'');
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}

/// 从 XML 声明 `<?xml ... encoding="GB2312"?>` 中取编码标签（只读响应头 200 字节）。
fn xml_encoding_label(bytes: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).to_ascii_lowercase();
    let pos = head.find("encoding=")?;
    let tail = &head[pos + "encoding=".len()..];
    let quote = tail.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = tail[1..].find(quote)?;
    let label = tail[1..1 + end].trim();
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}

/// 按「显式响应头 → Content-Type charset → XML 声明 → 严格 UTF-8 探测」的优先级解码。
///
/// `X-Lyric-Encoding` 是千千 Plus 取词时最可靠的信号；搜索走 charset / XML 声明；
/// 两者都缺时严格按 UTF-8 试，失败才回退 GBK（不 lossy，避免中文静默乱码）。
/// 显式声明 `encoding_rs` 依赖即为此处：`Cargo.lock` 原本只间接包含该 crate。
pub fn decode_bytes(
    bytes: &[u8],
    explicit_encoding: Option<&str>,
    content_type: Option<&str>,
    xml_declaration: bool,
) -> String {
    let mut label: Option<String> = explicit_encoding
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if label.is_none() {
        label = content_type.and_then(charset_label);
    }
    if label.is_none() && xml_declaration {
        label = xml_encoding_label(bytes);
    }

    let text = match label
        .as_deref()
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
    {
        Some(enc) => enc.decode_without_bom_handling(bytes).0.into_owned(),
        None => match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(_) => encoding_rs::GBK
                .decode_without_bom_handling(bytes)
                .0
                .into_owned(),
        },
    };
    text.trim_start_matches('\u{FEFF}').to_string()
}

/// 带超时与编码处理的 GET；返回解码后的文本。
async fn fetch_text(
    client: &reqwest::Client,
    url: String,
    xml_declaration: bool,
) -> Result<String, String> {
    let future = async {
        let resp = client
            .get(url.as_str())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        let headers = resp.headers().clone();
        let explicit = headers
            .get("x-lyric-encoding")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let content_type = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
        Ok(decode_bytes(
            &bytes,
            explicit.as_deref(),
            content_type.as_deref(),
            xml_declaration,
        ))
    };
    tokio::time::timeout(TIMEOUT, future)
        .await
        .map_err(|_| "在线歌词请求超时".to_string())?
}

/// XML 命名实体 + 数字实体解码（属性值里 `&amp;` `&#39;` 等）。
fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(semi) = after.find(';') else {
            out.push('&');
            rest = after;
            continue;
        };
        let ent = &after[..semi];
        let decoded = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32)
            }
            _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => out.push(c),
            None => {
                out.push('&');
                out.push_str(ent);
                out.push(';');
            }
        }
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    out
}

/// 取 XML 标签属性值；只认前面是空白的 `name=`，避免 `data-id=` 之类误命中。
fn xml_attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    for (i, _) in tag.match_indices(needle.as_str()) {
        if i > 0 && !tag.as_bytes()[i - 1].is_ascii_whitespace() {
            continue;
        }
        let start = i + needle.len();
        let quote = *tag.as_bytes().get(start)?;
        if quote != b'"' && quote != b'\'' {
            continue;
        }
        let from = start + 1;
        let end = tag[from..].find(quote as char)?;
        return Some(xml_unescape(&tag[from..from + end]));
    }
    None
}

/// 解析前的原始候选（搜索响应里的身份信息；正文惰性拉取）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLyricCandidate {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
}

/// 解析千千 Plus 搜索 XML（轻量扫描 `<lrc ... />`，不引入 XML crate）。
pub fn parse_qianqian_search(xml: &str) -> Vec<RawLyricCandidate> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(pos) = rest.find("<lrc") {
        rest = &rest[pos + "<lrc".len()..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        rest = &rest[end + 1..];
        let (id, title, artist) = (xml_attr(tag, "id"), xml_attr(tag, "title"), xml_attr(tag, "artist"));
        if let (Some(id), Some(title), Some(artist)) = (id, title, artist) {
            if !id.is_empty() && !title.is_empty() {
                out.push(RawLyricCandidate {
                    id,
                    title,
                    artist,
                    album: String::new(),
                });
            }
        }
    }
    out
}

fn value_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 解析 Rangotec 公开 JSON API：先判业务码 `code == 200`，再取 `data[]`。
/// `lrc` 全文随响应返回但按惰性拉取约定不随 IPC 带出，点选后再按 id 取词。
pub fn parse_rangotec_search(json: &serde_json::Value) -> Vec<RawLyricCandidate> {
    if value_text(&json["code"]).as_deref() != Some("200") {
        return Vec::new(); // HTTP 200 + code=400 是「无结果」，不能当成功
    }
    let Some(items) = json["data"].as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = value_text(&item["id"]).unwrap_or_default();
            let title = value_text(&item["title"]).unwrap_or_default();
            if id.is_empty() || title.is_empty() {
                return None;
            }
            Some(RawLyricCandidate {
                id,
                title,
                artist: value_text(&item["artist"]).unwrap_or_default(),
                album: value_text(&item["album"]).unwrap_or_default(),
            })
        })
        .collect()
}

/// 本地打分过滤：千千 Plus 无结果时会返回一堆不相关热门候选，必须按标题相关性剔除。
/// 复用主搜索的 `norm`（繁→简折叠 + 全角半角 + 小写，只作用于比较侧）与匹配权重。
pub fn rank_candidates(
    raw: Vec<RawLyricCandidate>,
    query_title: &str,
    query_artist: &str,
    provider: LyricsProvider,
    limit: usize,
) -> Vec<LyricCandidate> {
    let qt = searcher::norm(query_title);
    let qa = searcher::norm(query_artist);
    let mut seen: HashSet<String> = HashSet::new();
    let mut scored: Vec<LyricCandidate> = raw
        .into_iter()
        .filter_map(|r| {
            let t = searcher::norm(&r.title);
            let a = searcher::norm(&r.artist);
            let title_score = searcher::title_match(&qt, &t);
            if title_score <= 0.0 {
                return None; // 标题不相关 → 上游热门候选，丢弃
            }
            if !seen.insert(r.id.clone()) {
                return None;
            }
            Some(LyricCandidate {
                provider,
                id: r.id,
                title: r.title,
                artist: r.artist,
                album: r.album,
                score: title_score + searcher::artist_match(&qa, &a),
            })
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    scored
}

/// 增强型双时间戳 `[00:00:820,00:04:100]` → 标准 `[00:00.82]`（只取第一个时间戳）。
/// 标准 `[00:12.34]` 与元信息 `[ti:...]` 原样保留（冒号分段数不同）。
fn enhanced_to_standard(inner: &str) -> Option<String> {
    let first = inner.split(',').next()?;
    let mut parts = first.split(':');
    let mm = parts.next()?;
    let ss = parts.next()?;
    let ms = parts.next()?;
    if parts.next().is_some()
        || mm.is_empty()
        || ss.len() != 2
        || ms.is_empty()
        || ms.len() > 3
        || !mm.chars().all(|c| c.is_ascii_digit())
        || !ss.chars().all(|c| c.is_ascii_digit())
        || !ms.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let n: u32 = ms.parse().ok()?;
    // 3 位 = 毫秒 → 四舍五入到厘秒；2 位 = 厘秒；1 位 = 分秒的十分位。
    let cc = match ms.len() {
        3 => (n + 5) / 10,
        2 => n,
        _ => n * 10,
    }
    .min(99);
    Some(format!("[{mm}:{ss}.{cc:02}]"))
}

/// 逐段扫描一行里所有 `[...]` 标签，把增强型时间戳换成标准 LRC 时间戳。
pub fn normalize_enhanced_timestamps(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else {
            out.push('[');
            rest = after;
            continue;
        };
        let inner = &after[..close];
        match enhanced_to_standard(inner) {
            Some(ts) => out.push_str(&ts),
            None => {
                out.push('[');
                out.push_str(inner);
                out.push(']');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

fn is_watermark(line: &str) -> bool {
    let lower = line.to_lowercase();
    WATERMARK_MARKERS.iter().any(|m| lower.contains(*m))
}

/// LRC 清洗（两源共用）：统一换行 → 去空元信息标签 → 去署名/版权/广告行 →
/// 增强型时间戳转标准 → 去首尾空行 → 以单个换行结尾。空结果返回空串（前端按取词失败处理）。
pub fn clean_lrc(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut kept: Vec<String> = Vec::new();
    for line in normalized.split('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            kept.push(String::new());
            continue;
        }
        if EMPTY_META_TAGS
            .iter()
            .any(|tag| trimmed.eq_ignore_ascii_case(*tag))
        {
            continue;
        }
        if is_watermark(trimmed) {
            continue;
        }
        kept.push(normalize_enhanced_timestamps(trimmed));
    }
    while matches!(kept.first(), Some(s) if s.is_empty()) {
        kept.remove(0);
    }
    while matches!(kept.last(), Some(s) if s.is_empty()) {
        kept.pop();
    }
    if kept.is_empty() {
        return String::new();
    }
    let mut out = kept.join("\n");
    out.push('\n');
    out
}

/// 千千 Plus 客户端（base_url 可注入，供 `tests/` mock server 单测）。
pub struct Qianqian {
    pub base_url: String,
}

impl Default for Qianqian {
    fn default() -> Self {
        Self {
            base_url: QIANQIAN_BASE.to_string(),
        }
    }
}

impl Qianqian {
    pub async fn search(
        &self,
        client: &reqwest::Client,
        title: &str,
        artist: &str,
    ) -> Vec<RawLyricCandidate> {
        let url = format!(
            "{}?sh&Title={}&Artist={}",
            self.base_url,
            pct_encode(title),
            pct_encode(artist)
        );
        match fetch_text(client, url, true).await {
            Ok(xml) => parse_qianqian_search(&xml),
            Err(_) => Vec::new(), // 单源失败降级空结果，不影响另一源
        }
    }

    pub async fn fetch(&self, client: &reqwest::Client, id: &str) -> Option<String> {
        let url = format!("{}?dl&Id={}", self.base_url, pct_encode(id));
        fetch_text(client, url, false).await.ok()
    }
}

/// Rangotec 客户端（`od=asc` 取老数据；`search_url` / `lyric_url_base` 可注入供单测）。
pub struct Rangotec {
    pub search_url: String,
    pub lyric_url_base: String,
}

impl Default for Rangotec {
    fn default() -> Self {
        Self {
            search_url: RANGOTEC_SEARCH.to_string(),
            lyric_url_base: RANGOTEC_LYRIC.to_string(),
        }
    }
}

impl Rangotec {
    pub async fn search(
        &self,
        client: &reqwest::Client,
        title: &str,
        artist: &str,
    ) -> Vec<RawLyricCandidate> {
        let url = match reqwest::Url::parse_with_params(
            &self.search_url,
            &[("title", title), ("artist", artist), ("od", "asc")],
        ) {
            Ok(u) => u.to_string(),
            Err(_) => return Vec::new(),
        };
        match fetch_text(client, url, false).await {
            Ok(body) => serde_json::from_str::<serde_json::Value>(&body)
                .map(|json| parse_rangotec_search(&json))
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    /// 老协议兼容下载动作：`?dl?Id=<id>&Code=abc`（`/qianqian` 的 `sh` 搜索对外不可用，
    /// 但 `dl` 实测稳定返回与 JSON API `lrc` 字段同一份文本）。
    pub async fn fetch(&self, client: &reqwest::Client, id: &str) -> Option<String> {
        let url = format!("{}?dl?Id={}&Code=abc", self.lyric_url_base, pct_encode(id));
        fetch_text(client, url, false).await.ok()
    }
}

/// 两源并发在线搜词（`search_lyrics` command 的实现）。
///
/// 空 title → 直接返回空列表，不发请求（与裸文件不自动搜同理）。两源各自 6s 超时，
/// 任一源失败只降级为空，另一源结果照常返回；不写任何 `isOffline` / `all_failed` 状态。
pub async fn search_lyrics(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
) -> Vec<LyricCandidate> {
    let t = sanitize_query(title);
    if t.is_empty() {
        return Vec::new();
    }
    let a = sanitize_query(artist);
    let (qianqian, rangotec) = tokio::join!(
        Qianqian::default().search(client, &t, &a),
        Rangotec::default().search(client, &t, &a),
    );
    let mut out = rank_candidates(qianqian, &t, &a, LyricsProvider::Qianqian, PER_PROVIDER_TOP);
    out.extend(rank_candidates(
        rangotec,
        &t,
        &a,
        LyricsProvider::Rangotec,
        PER_PROVIDER_TOP,
    ));
    out
}

/// 点选在线歌词候选后按 id 取正文（`fetch_online_lyric` command 的实现）。
/// 取回后立即清洗；清洗后为空 → `None`（前端按「该候选取词为空」提示并可换一条）。
pub async fn fetch_online_lyric(
    client: &reqwest::Client,
    provider: LyricsProvider,
    id: &str,
) -> Option<String> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let raw = match provider {
        LyricsProvider::Qianqian => Qianqian::default().fetch(client, id).await,
        LyricsProvider::Rangotec => Rangotec::default().fetch(client, id).await,
    }?;
    let cleaned = clean_lrc(&raw);
    if cleaned.trim().is_empty() {
        None
    } else {
        Some(cleaned)
    }
}
