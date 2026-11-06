// MusicTag — 发行年份换算（FR-8 #16/#19，零新增依赖）。
//
// 教训来自批量年份脚本：
// - 网易云 `album.publishTime = 0` 是**缺失值**，换算会得到假的 1970 年，必须先跳过；
// - Windows 的 `datetime.fromtimestamp()` 对 1970 年前的负时间戳直接报错，
//   故这里用纯整数公历换算（Howard Hinnant `civil_from_days`），负天数同样成立；
// - 年份须落在 1900 ~ 次年之间，脏日期/预发行占位一律丢弃（宁缺毋滥）。

use std::time::{SystemTime, UNIX_EPOCH};

/// 允许年份下限（更早的发行日期基本是脏数据）。
pub const MIN_YEAR: i64 = 1900;

/// 一天的秒数。
const SECS_PER_DAY: i64 = 86_400;

/// 当前 UTC 年份 + 1（允许年末预发行 / 次年上架）。
///
/// `SystemTime::now()` 早于 UNIX_EPOCH 的异常情形按 0 秒兜底，再由 `plausible` 收敛。
pub fn max_year() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    civil_from_days(secs.div_euclid(SECS_PER_DAY)).0 + 1
}

/// 年份是否可信（1900 ~ 次年）。
pub fn plausible(year: i64) -> bool {
    year >= MIN_YEAR && year <= max_year()
}

/// 天数（相对 1970-01-01，可为负）→ 公历 `(年, 月, 日)`。
///
/// 纯整数实现，不依赖 chrono / libc 本地时间；`div_euclid` 保证 1970 年前也正确。
/// 参考 Howard Hinnant `civil_from_days`（400 年 era + 移位纪元 719468）。
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// 秒级 Unix 时间戳（QQ `pubtime`）→ 年份字符串。
///
/// `0` 是缺失值，直接拒绝（否则换算成假的 1970）；负秒数（1970 年前）允许，
/// 由 `plausible` 决定是否保留。
pub fn year_from_unix_secs(secs: i64) -> Option<String> {
    if secs == 0 {
        return None;
    }
    let year = civil_from_days(secs.div_euclid(SECS_PER_DAY)).0;
    if plausible(year) {
        Some(year.to_string())
    } else {
        None
    }
}

/// 毫秒级 Unix 时间戳（网易云 `album.publishTime`）→ 年份字符串。
///
/// `publishTime = 0` / 非正数一律视为缺失，绝不换算成 1970。
pub fn year_from_unix_millis(ms: i64) -> Option<String> {
    if ms <= 0 {
        return None;
    }
    year_from_unix_secs(ms / 1000)
}

/// ISO 日期字符串 → 年份（iTunes `releaseDate`、Deezer `album.release_date`、
/// MusicBrainz `first-release-date` / `releases[].date`、QQ `album.time_public`）。
///
/// 只取开头连续 4 位数字并校验范围；`"0000-00-00"`、`""`、噪声文本一律 `None`。
/// 注意年份可能只有年份本身（MusicBrainz 常返回 `"1998"`），故不要求必须有分隔符。
pub fn year_from_date_str(raw: &str) -> Option<String> {
    let prefix: String = raw
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if prefix.len() < 4 {
        return None;
    }
    let year: i64 = prefix[..4].parse().ok()?;
    if plausible(year) {
        Some(year.to_string())
    } else {
        None
    }
}
