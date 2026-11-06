// MusicTag — 搜索 command 薄壳（design.md §10 分层规范，v1-search-backend / v1-search-fixes /
// search-sources-renewal / search-cover-album）。
//
// 只做参数接收与对 `service::searcher` 委托，不含加密/HTTP/聚合逻辑：
// - `search_song(title, artist, album)` → 五源并发搜索 + 打分去重（候选秒出，歌词/封面惰性拉取）；
// - `search_source(source, title, artist, album)` → 单源搜索原始候选（C2 换源用，绕过聚合去重）；
// - `fetch_lyric(source, id)` → 点选歌词候选拉文本（None = 取词失败，前端换源 C2）；
// - `download_cover(url)` → 点选封面缩略图下载（5s 超时 + 12MB 限流）；
// - `search_metadata_fallback(title, artist, album)` → 用户手动点「补全年份」才发起的
//   分级兜底查询（FR-8 #18/#19，离线不触发），不进常规五源并发链。
// 五个 command 均为 async：Tauri 在异步运行时执行，不阻塞 WebView 主线程（FR-8.12a）。

use crate::model::{MetadataCandidate, MusicSourceId, SearchResult, SongCandidate};
use crate::service::searcher;

/// 五源并发搜索（`String, String, String → SearchResult`，契约 §10.3）。
///
/// `album`（search-cover-album）：综合进查询关键词与打分；空 → 回退 title(+artist) 现行为。
#[tauri::command]
pub async fn search_song(title: String, artist: String, album: String) -> SearchResult {
    searcher::search_song(searcher::client(), &title, &artist, &album).await
}

/// 单源搜索（`MusicSourceId, String, String, String → Vec<SongCandidate>`，契约 §10.3 v1-search-fixes）。
///
/// 不走跨源聚合去重：C2 换源需要「其他来源对同一首歌的候选」（聚合会把同曲多源折叠成一条，
/// 导致换源找不到其他源）；失败/超时 → 空列表，前端跳过该源。
#[tauri::command]
pub async fn search_source(
    source: MusicSourceId,
    title: String,
    artist: String,
    album: String,
) -> Vec<SongCandidate> {
    searcher::search_source(searcher::client(), source, &title, &artist, &album).await
}

/// 点选歌词候选拉文本（`MusicSourceId, String → Option<String>`，契约 §10.3）。
#[tauri::command]
pub async fn fetch_lyric(source: MusicSourceId, id: String) -> Option<String> {
    searcher::fetch_lyric(searcher::client(), source, &id).await
}

/// 点选封面缩略图下载（`String → Vec<u8>`，契约 §10.3；mime 解析/压缩/转 data URL 在前端）。
#[tauri::command]
pub async fn download_cover(url: String) -> Result<Vec<u8>, String> {
    searcher::download_cover(searcher::client(), &url).await
}

/// 分级兜底年份查询（`String, String, String → Vec<MetadataCandidate>`，契约 §10.3 / FR-8 #18）。
///
/// 仅由前端「补全年份」按钮手动触发：前端已判定本次常规搜索 `all_failed=false`、
/// 候选无可信年份、当前年份为空且非只读；若本首尚未跑过常规搜索，前端先补跑
/// `search_song` 再按同一判据决定是否调用本命令；`all_failed=true`（离线）时不调用。
/// 一级同平台备用端点无可信结果才跑二级（Deezer / MusicBrainz 1 req/s），命中即停。
/// 返回候选只带文本与匹配度，不落盘、不自动写盘。
#[tauri::command]
pub async fn search_metadata_fallback(
    title: String,
    artist: String,
    album: String,
) -> Vec<MetadataCandidate> {
    searcher::fallback::search_metadata_fallback(searcher::client(), &title, &artist, &album).await
}
