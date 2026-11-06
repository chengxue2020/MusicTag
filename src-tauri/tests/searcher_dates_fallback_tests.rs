// MusicTag — 发行年份换算 + 分级兜底打分单测（metadata-year-fallback，rust-tests-separation 外置）。
//
// 只测纯函数，不发任何网络请求（HTTP 层沿用既有 `searcher_*_tests.rs` 的 mock 手法）。
// 三条教训来自批量年份脚本，必须在此钉死，防止兜底逻辑重蹈覆辙：
// - 网易云 `publishTime = 0` 是缺失值，换算会得到**假的 1970 年**；
// - 窄口径 `norm`（只留 `[0-9a-z\u4e00-\u9fff]`）会把俄文 / 日文假名 / 韩文整段清空，
//   相似度恒为 0；
// - 歌名零关联的低置信同名结果必须丢弃（宁缺毋滥），年份须落在 1900 ~ 次年。
//
// 访问被测类型经 `app_lib::service::searcher::`（Cargo.toml `[lib] name = "app_lib"`）。

use app_lib::service::searcher::dates;
use app_lib::service::searcher::fallback::{clean_title, finalize, similarity, wide_norm, RawHit};

fn hit(year: &str, title: &str, artist: &str) -> RawHit {
    RawHit {
        year: year.to_string(),
        title: title.to_string(),
        artist: artist.to_string(),
        album: String::new(),
        source: "测试源",
    }
}

// ── dates：纯整数公历换算 ────────────────────────────────────────────────

#[test]
fn civil_from_days_epoch_and_negative_day() {
    assert_eq!(dates::civil_from_days(0), (1970, 1, 1));
    // 1970 年前负天数同样成立（Windows datetime.fromtimestamp 在此会报错）。
    assert_eq!(dates::civil_from_days(-1), (1969, 12, 31));
}

#[test]
fn zero_timestamp_is_missing_not_1970() {
    // QQ pubtime=0 / 网易云 publishTime=0 都是缺失值，绝不产出假 1970。
    assert_eq!(dates::year_from_unix_secs(0), None);
    assert_eq!(dates::year_from_unix_millis(0), None);
    assert_eq!(dates::year_from_unix_millis(-1), None);
}

#[test]
fn unix_seconds_and_millis_map_to_same_year() {
    // QQ `pubtime`（秒级）与网易云 `publishTime`（毫秒级）。
    assert_eq!(dates::year_from_unix_secs(1_466_697_600), Some("2016".to_string()));
    assert_eq!(
        dates::year_from_unix_millis(1_466_697_600_000),
        Some("2016".to_string())
    );
}

#[test]
fn date_str_accepts_iso_and_bare_year() {
    // iTunes releaseDate / Deezer album.release_date / MusicBrainz first-release-date。
    assert_eq!(dates::year_from_date_str("2014-11-07"), Some("2014".to_string()));
    assert_eq!(dates::year_from_date_str("1998"), Some("1998".to_string()));
}

#[test]
fn date_str_rejects_placeholder_and_out_of_range() {
    assert_eq!(dates::year_from_date_str(""), None);
    assert_eq!(dates::year_from_date_str("abc2014"), None);
    assert_eq!(dates::year_from_date_str("0000-00-00"), None);
    assert_eq!(dates::year_from_date_str("1899-12-31"), None);
    assert_eq!(dates::year_from_date_str("3000-01-01"), None);
}

// ── fallback：归一化 / 相似度 / 打分收敛 ─────────────────────────────────

#[test]
fn wide_norm_keeps_non_chinese_scripts() {
    let ru = wide_norm("Агата Кристи");
    assert!(ru.contains("агата"), "俄文被清空: {ru:?}");
    let ja = wide_norm("メロディー");
    assert!(ja.contains('メ'), "日文假名被清空: {ja:?}");
    let ko = wide_norm("사랑해");
    assert!(ko.contains("사랑해"), "韩文被清空: {ko:?}");
}

#[test]
fn clean_title_removes_brackets_and_version_tags() {
    let t = clean_title("千里之外 (Live) [Remix]");
    assert!(!t.contains('('), "括号内容未去除: {t:?}");
    assert!(!t.contains("live"), "版本后缀未去除: {t:?}");
    assert!(!t.contains("remix"), "版本后缀未去除: {t:?}");
    assert!(t.contains("千里之外"));
}

#[test]
fn similarity_is_ratio_of_lcs() {
    assert!((similarity("abc", "abc") - 1.0).abs() < f32::EPSILON);
    assert_eq!(similarity("abc", "xyz"), 0.0);
    assert_eq!(similarity("", "abc"), 0.0);
}

#[test]
fn finalize_drops_unrelated_title_and_dirty_year() {
    let out = finalize(
        "千里之外",
        "周杰伦",
        vec![
            hit("2006", "完全无关的歌", "某人"), // 歌名零关联
            hit("0000", "千里之外", "周杰伦"),   // 占位年份
            hit("1899", "千里之外", "周杰伦"),   // 越界年份
            hit("2006", "千里之外", "周杰伦"),   // 唯一可信命中
        ],
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].year, "2006");
    assert_eq!(out[0].source, "测试源");
}

#[test]
fn finalize_dedups_by_year_and_title_caps_at_eight() {
    let dedup = finalize(
        "歌名",
        "歌手",
        vec![
            hit("2006", "歌名", "歌手"),
            hit("2006", "歌名", "歌手"),
            hit("2007", "歌名", "歌手"),
        ],
    );
    assert_eq!(dedup.len(), 2);

    let many: Vec<RawHit> = (2000..2012)
        .map(|y| hit(&y.to_string(), "歌名", "歌手"))
        .collect();
    assert_eq!(finalize("歌名", "歌手", many).len(), 8);
}

#[test]
fn finalize_sorts_by_score_and_penalizes_artist_mismatch() {
    let ranked = finalize(
        "千里之外",
        "周杰伦",
        vec![
            hit("2006", "千里", "周杰伦"),       // 部分匹配
            hit("2007", "千里之外", "周杰伦"),  // 完全匹配
        ],
    );
    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].year, "2007");
    assert!(ranked[0].score >= ranked[1].score);

    let same = finalize("千里之外", "周杰伦", vec![hit("2006", "千里之外", "周杰伦")]);
    let diff = finalize("千里之外", "周杰伦", vec![hit("2006", "千里之外", "完全无关")]);
    assert_eq!(same.len(), 1);
    assert_eq!(diff.len(), 1);
    assert!(same[0].score > diff[0].score, "歌手不匹配应扣分");
}
