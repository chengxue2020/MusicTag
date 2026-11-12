// MusicTag — `service/lyrics_online.rs` 集成测试（rust-tests-separation 外置）。
//
// 覆盖 online-lyrics-sources：
// - 编码解码（X-Lyric-Encoding / charset / XML 声明 / 严格 UTF-8 探测 / GBK 回退 / BOM）；
// - 千千 Plus XML 解析（含实体、无值 action 裸参数）、Rangotec JSON 业务码判定；
// - 标题相关性打分过滤（上游无结果时返回的热门候选）；
// - LRC 清洗（用户要求去掉千千 Plus 署名/版权声明、增强型双时间戳、广告行）；
// - 两个 provider 的搜索 / 取词 URL 形状与单源失败降级。
// 全部走 `tests/common` mock HTTP server，不依赖外网；生产 `src/` 零 `#[cfg(test)]`。

mod common;

use app_lib::model::{LyricCandidate, LyricsProvider};
use app_lib::service::lyrics_online::{
    clean_lrc, decode_bytes, normalize_enhanced_timestamps, parse_qianqian_search,
    parse_rangotec_search, pct_encode, rank_candidates, sanitize_query, search_lyrics,
    Qianqian, Rangotec, RawLyricCandidate,
};
use common::{mock_http_capture, mock_http_once};

fn gbk_bytes(s: &str) -> Vec<u8> {
    let (encoded, _, _) = encoding_rs::GBK.encode(s);
    encoded.into_owned()
}

fn http_response(headers: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 200 OK\r\n{headers}\r\n\r\n").into_bytes();
    out.extend_from_slice(body);
    out
}

// ---- 查询词清洗与百分号编码 ----

#[test]
fn sanitize_query_strips_control_and_blocked_chars() {
    assert_eq!(sanitize_query("  可能 \t\n "), "可能");
    assert_eq!(sanitize_query("A&B / (Live) [Remix]"), "A&B / (Live) Remix");
    assert_eq!(sanitize_query("  "), "");
}

#[test]
fn pct_encode_percent_encodes_utf8_and_unsafe_bytes() {
    assert_eq!(pct_encode("可能 程响"), "%E5%8F%AF%E8%83%BD%20%E7%A8%8B%E5%93%8D");
    assert_eq!(pct_encode("A&B/x?y"), "A%26B%2Fx%3Fy");
    assert_eq!(pct_encode("a-_.~9Z"), "a-_.~9Z");
}

// ---- 编码解码 ----

#[test]
fn decode_bytes_uses_explicit_encoding_header_first() {
    let bytes = gbk_bytes("[00:00.00]中文歌词\n");
    assert_eq!(
        decode_bytes(&bytes, Some("gbk"), Some("text/plain; charset=utf-8"), false),
        "[00:00.00]中文歌词\n"
    );
}

#[test]
fn decode_bytes_uses_content_type_charset_then_xml_declaration() {
    let xml = "<?xml version=\"1.0\" encoding=\"GB2312\"?><result><lrc id=\"1\" artist=\"程响\" title=\"可能\"/></result>";
    let bytes = gbk_bytes(xml);
    // 无显式头 → Content-Type charset 优先
    assert!(decode_bytes(&bytes, None, Some("text/xml; charset=GBK"), true).contains("可能"));
    // 无 Content-Type → 回落到 XML 声明
    assert!(decode_bytes(&bytes, None, None, true).contains("可能"));
}

#[test]
fn decode_bytes_strict_utf8_probe_then_gbk_fallback_and_strips_bom() {
    assert_eq!(decode_bytes("中文".as_bytes(), None, None, false), "中文");
    let mut with_bom = vec![0xEF, 0xBB, 0xBF];
    with_bom.extend_from_slice("[00:00.00]歌词".as_bytes());
    assert_eq!(decode_bytes(&with_bom, None, None, false), "[00:00.00]歌词");
    let gbk = gbk_bytes("[00:00.00]可能");
    assert_eq!(decode_bytes(&gbk, None, None, false), "[00:00.00]可能");
}

// ---- 千千 Plus XML 解析 ----

#[test]
fn parse_qianqian_search_reads_attributes_and_unescapes_entities() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<result>
  <lrc id="48734" artist="程响" title="可能"></lrc>
  <lrc id="53904" artist="A &amp; B" title="是否 &quot;Live&quot; &#39;版&#39;"></lrc>
  <lrc id="" artist="x" title="缺 id 应丢弃"></lrc>
</result>"#;
    let list = parse_qianqian_search(xml);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, "48734");
    assert_eq!(list[0].artist, "程响");
    assert_eq!(list[0].title, "可能");
    assert_eq!(list[0].album, "");
    assert_eq!(list[1].artist, "A & B");
    assert_eq!(list[1].title, "是否 \"Live\" '版'");
}

#[test]
fn parse_qianqian_search_empty_result_returns_empty() {
    assert!(parse_qianqian_search("<result></result>").is_empty());
    assert!(parse_qianqian_search("").is_empty());
}

// ---- Rangotec JSON 业务码 ----

#[test]
fn parse_rangotec_search_requires_business_code_200() {
    let no_result = serde_json::json!({"code": 400, "msg": "无结果", "data": []});
    assert!(parse_rangotec_search(&no_result).is_empty());

    let ok = serde_json::json!({
        "code": 200,
        "data": [
            {"id": 446826, "artist": "程响", "title": "可能", "album": "可能", "lrc": "[00:00.00]歌词"},
            {"id": "446827", "artist": "程响", "title": "可能 (Live)", "album": "", "lrc": ""},
            {"id": 0, "artist": "x", "title": "", "album": ""}
        ]
    });
    let list = parse_rangotec_search(&ok);
    assert_eq!(list.len(), 2, "缺 id/title 的条目应丢弃");
    assert_eq!(list[0].id, "446826", "数字 id 应转字符串");
    // 上游把歌名直接填进 album，视为无专辑（避免专辑列重复显示歌名）。
    assert_eq!(list[0].album, "");
    // RawLyricCandidate 只有身份字段，lrc 全文不随解析结果带出（惰性拉取）
    assert_eq!(list[0], RawLyricCandidate {
        id: "446826".into(),
        title: "可能".into(),
        artist: "程响".into(),
        album: "".into(),
    });
}

#[test]
fn parse_rangotec_search_cleans_placeholder_identity_fields() {
    let json = serde_json::json!({
        "code": 200,
        "data": [
            {"id": 446826, "artist": "None", "title": "可能", "album": "None"},
            {"id": 446827, "artist": "", "title": "可能", "album": "可能"},
            {"id": 446828, "artist": "程响", "title": "可能", "album": " null "},
            {"id": "None", "artist": "程响", "title": "可能", "album": ""}
        ]
    });
    let list = parse_rangotec_search(&json);
    assert_eq!(list.len(), 3, "id 为占位值的条目应丢弃");
    assert_eq!(list[0].artist, "", "字符串 None 应清洗为空");
    assert_eq!(list[0].album, "");
    assert_eq!(list[1].artist, "");
    assert_eq!(list[1].album, "", "专辑等于歌名应置空");
    assert_eq!(list[2].artist, "程响", "正常歌手名不受影响");
    assert_eq!(list[2].album, "", "带空白的 null 也应清洗");
}

// ---- 打分过滤 ----

#[test]
fn rank_candidates_drops_irrelevant_hot_candidates_and_dedupes() {
    let raw = vec![
        RawLyricCandidate { id: "1".into(), title: "可能".into(), artist: "程响".into(), album: "".into() },
        RawLyricCandidate { id: "1".into(), title: "可能".into(), artist: "程响".into(), album: "".into() },
        RawLyricCandidate { id: "2".into(), title: "可能 (Live)".into(), artist: "程响".into(), album: "".into() },
        RawLyricCandidate { id: "3".into(), title: "四季予你".into(), artist: "程响".into(), album: "".into() },
        RawLyricCandidate { id: "4".into(), title: "念".into(), artist: "程响".into(), album: "".into() },
    ];
    let list = rank_candidates(raw, "可能", "程响", LyricsProvider::Qianqian, 8);
    assert_eq!(list.len(), 2, "只保留标题相关且去重后的候选");
    assert_eq!(list[0].id, "1");
    assert!(list[0].score > list[1].score, "精确标题匹配分数更高");
    assert_eq!(list[0].provider, LyricsProvider::Qianqian);
}

#[test]
fn rank_candidates_respects_limit() {
    let raw: Vec<RawLyricCandidate> = (0..20)
        .map(|i| RawLyricCandidate {
            id: i.to_string(),
            title: format!("可能 {i}"),
            artist: "程响".into(),
            album: "".into(),
        })
        .collect();
    assert_eq!(rank_candidates(raw, "可能", "程响", LyricsProvider::Rangotec, 8).len(), 8);
}

#[test]
fn rank_candidates_dedupes_rangotec_identical_identity() {
    let mut raw: Vec<RawLyricCandidate> = (0..6)
        .map(|i| RawLyricCandidate {
            id: i.to_string(),
            title: "可能".into(),
            artist: "程响".into(),
            album: "".into(),
        })
        .collect();
    raw.push(RawLyricCandidate {
        id: "9".into(),
        title: "可能".into(),
        artist: "李怡然同学".into(),
        album: "".into(),
    });
    let list = rank_candidates(raw, "可能", "程响", LyricsProvider::Rangotec, 8);
    assert_eq!(list.len(), 2, "同身份只留一条，不同歌手版本仍保留");
    assert_eq!(list[0].id, "0", "保留上游 od=asc 顺序的第一条");
    assert_eq!(list[1].artist, "李怡然同学");
}

#[test]
fn rank_candidates_keeps_qianqian_distinct_ids() {
    let raw: Vec<RawLyricCandidate> = (0..3)
        .map(|i| RawLyricCandidate {
            id: i.to_string(),
            title: "可能".into(),
            artist: "程响".into(),
            album: "".into(),
        })
        .collect();
    assert_eq!(
        rank_candidates(raw, "可能", "程响", LyricsProvider::Qianqian, 8).len(),
        3,
        "千千 Plus 维持只按 id 去重的既有行为"
    );
}

// ---- LRC 清洗（用户决定：去掉千千 Plus 署名/版权声明） ----

#[test]
fn clean_lrc_removes_qianqian_attribution_and_copyright_lines() {
    let raw = "[ti:可能]\r\n[ar:程响]\r\n[al:可能]\r\n[by:QianQian.Plus]\r\n\r\n\
[00:00.01]本歌词由 QianQian.Plus 提供。\r\n\
[00:00.02]来源于网络，版权归原作者及版权方所有。\r\n\
[00:00.05]可能 - 程响\r\n\
[00:10.12]可能南方的阳光 照着北方的风\r\n";
    let cleaned = clean_lrc(raw);
    assert_eq!(
        cleaned,
        "[ti:可能]\n[ar:程响]\n[al:可能]\n[00:00.05]可能 - 程响\n[00:10.12]可能南方的阳光 照着北方的风\n"
    );
    assert!(!cleaned.to_lowercase().contains("qianqian.plus"));
    assert!(!cleaned.contains("版权"));
}

#[test]
fn clean_lrc_converts_enhanced_timestamps_and_filters_ads() {
    let raw = "[ti:]\r\n[ar:]\r\n[al:]\r\n[by:]\r\n\r\n\
[00:00:680,00:01:278]「未经著作权人书面许可 不得以任何方式（包括翻唱、翻录等）使用」\r\n\
[00:00:820,00:04:100]可能西安城墙上有人誓言不分\r\n\
[00:04:400,00:07:950]本歌曲翻唱由 Vemus 未音提供生产能力\r\n\
[00:12.34]标准时间轴原样保留\r\n";
    let cleaned = clean_lrc(raw);
    assert_eq!(
        cleaned,
        "[00:00.82]可能西安城墙上有人誓言不分\n[00:12.34]标准时间轴原样保留\n"
    );
}

#[test]
fn normalize_enhanced_timestamps_keeps_standard_lrc_and_meta_tags() {
    assert_eq!(normalize_enhanced_timestamps("[00:12.34]歌词"), "[00:12.34]歌词");
    assert_eq!(normalize_enhanced_timestamps("[ti:专辑]"), "[ti:专辑]");
    assert_eq!(
        normalize_enhanced_timestamps("[01:02:345,01:05:678][01:06:000]歌词"),
        "[01:02.35][01:06.00]歌词"
    );
}

#[test]
fn clean_lrc_empty_or_all_watermark_returns_empty() {
    assert_eq!(clean_lrc(""), "");
    assert_eq!(clean_lrc("\r\n\r\n"), "");
    assert_eq!(clean_lrc("[by:QianQian.Plus]\r\n[00:00.01]本歌词由 QianQian.Plus 提供。"), "");
}

// ---- 契约序列化 ----

#[test]
fn lyrics_provider_and_candidate_serialize_to_contract_shape() {
    let json = serde_json::to_string(&LyricCandidate {
        provider: LyricsProvider::Qianqian,
        id: "48734".into(),
        title: "可能".into(),
        artist: "程响".into(),
        album: "".into(),
        score: 0.9,
    })
    .unwrap();
    assert!(json.contains(r#""provider":"qianqian""#), "实际: {json}");
    assert!(json.contains(r#""score":0.9"#), "实际: {json}");
    assert!(!json.contains("Qianqian"));
    assert_eq!(
        serde_json::to_string(&LyricsProvider::Rangotec).unwrap(),
        r#""rangotec""#
    );
}

// ---- HTTP 形状与单源失败降级 ----

#[tokio::test]
async fn qianqian_search_uses_bare_action_and_percent_encoded_params() {
    let response = http_response(
        "Content-Type: text/xml; charset=UTF-8",
        r#"<?xml version="1.0" encoding="UTF-8"?><result><lrc id="48734" artist="程响" title="可能"/></result>"#.as_bytes(),
    );
    let (url, captured) = mock_http_capture(response);
    let source = Qianqian { base_url: url.clone() };
    let list = source.search(&reqwest::Client::new(), "可能", "程响").await;
    assert_eq!(list.len(), 1);

    let target = captured.lock().unwrap().clone();
    // 请求行第 2 段是 origin-form，url crate 会把空 path 规范成 `/`，故前缀是 `/?sh&...`。
    assert!(target.contains("?sh&Title="), "action 必须是无值裸参数 sh，实际: {target}");
    assert!(!target.contains("sh="), "裸参数不得退化成 sh=，实际: {target}");
    let parsed = reqwest::Url::parse(&format!("{url}{target}")).unwrap();
    let params: std::collections::HashMap<String, String> = parsed.query_pairs().into_owned().collect();
    assert_eq!(params.get("Title").map(String::as_str), Some("可能"));
    assert_eq!(params.get("Artist").map(String::as_str), Some("程响"));
}

#[tokio::test]
async fn qianqian_fetch_decodes_gbk_via_lyric_encoding_header() {
    let body = gbk_bytes("[00:00.00]中文歌词\n");
    let response = http_response("Content-Type: text/plain\r\nX-Lyric-Encoding: gbk", &body);
    let url = mock_http_once(response);
    let source = Qianqian { base_url: url };
    let text = source.fetch(&reqwest::Client::new(), "48734").await.unwrap();
    assert_eq!(text, "[00:00.00]中文歌词\n");
    assert_eq!(clean_lrc(&text), "[00:00.00]中文歌词\n");
}

#[tokio::test]
async fn rangotec_search_uses_public_json_api_with_asc_order() {
    let response = http_response(
        "Content-Type: application/json",
        r#"{"code":200,"data":[{"id":446826,"artist":"程响","title":"可能","album":"","lrc":"[00:00.00]x"}]}"#.as_bytes(),
    );
    let (url, captured) = mock_http_capture(response);
    let source = Rangotec {
        search_url: url.clone(),
        lyric_url_base: url.clone(),
    };
    let list = source.search(&reqwest::Client::new(), "可能", "程响").await;
    assert_eq!(list.len(), 1);

    let target = captured.lock().unwrap().clone();
    let parsed = reqwest::Url::parse(&format!("{url}{target}")).unwrap();
    let params: std::collections::HashMap<String, String> = parsed.query_pairs().into_owned().collect();
    assert_eq!(params.get("title").map(String::as_str), Some("可能"));
    assert_eq!(params.get("artist").map(String::as_str), Some("程响"));
    assert_eq!(params.get("od").map(String::as_str), Some("asc"), "默认只用老数据排序");
}

#[tokio::test]
async fn rangotec_fetch_uses_legacy_download_action() {
    let response = http_response(
        "Content-Type: text/xml;charset=UTF-8",
        b"[00:00.00]\xe7\xa8\x8b\xe5\x93\x8d - \xe5\x8f\xaf\xe8\x83\xbd\n",
    );
    let (url, captured) = mock_http_capture(response);
    let source = Rangotec {
        search_url: url.clone(),
        lyric_url_base: url.clone(),
    };
    let text = source.fetch(&reqwest::Client::new(), "446826").await.unwrap();
    assert_eq!(text, "[00:00.00]程响 - 可能\n");
    let target = captured.lock().unwrap().clone();
    assert!(target.contains("dl?Id=446826"), "实际: {target}");
    assert!(target.contains("Code=abc"), "实际: {target}");
}

#[tokio::test]
async fn provider_search_degrades_to_empty_on_http_error() {
    let response = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec();
    let url = mock_http_once(response);
    let source = Qianqian { base_url: url };
    assert!(source.search(&reqwest::Client::new(), "可能", "程响").await.is_empty());
}

#[tokio::test]
async fn search_lyrics_empty_title_does_not_send_request() {
    let client = reqwest::Client::new();
    assert!(search_lyrics(&client, "", "程响").await.is_empty());
    assert!(search_lyrics(&client, "   ", "").await.is_empty());
}
