// Tauri command 类型化封装（design.md §10 前端 api 层：组件零 invoke 直呼，IPC 全走此层）。
//
// v1-search-backend 搜索契约（commands/search.rs、model.rs）：search_song / fetch_lyric /
// download_cover。online-lyrics-sources 另加 search_lyrics / fetch_online_lyric 两个
// **独立在线歌词** command（与五源主搜索平行但不交叉）。实现 = `invokeCommand` 透传
// （同 songs.ts 模式），候选生命周期（选中即搜、切歌即弃、离线、C2、静默忽略）
// 逻辑在 store/song.ts，不在 api 层。
import { invokeCommand } from './client'
import type {
  LyricCandidate,
  LyricsProvider,
  MetadataCandidate,
  MusicSourceId,
  SearchResult,
  SongCandidate,
} from './types'

/** 五源并发搜索（标题 + 作者 + 专辑 → 打分去重候选；空 title 由后端守卫过滤 → 空态）。
 *  album（search-cover-album）：综合进各源查询关键词与打分；空 → 回退 title(+artist) 现行为。 */
export function searchSongs(title: string, artist: string, album: string): Promise<SearchResult> {
  return invokeCommand<SearchResult>('search_song', { title, artist, album })
}

/** 单源搜索原始候选（`MusicSourceId, String, String, String → SongCandidate[]`；C2 换源用，绕过聚合去重）。 */
export function searchSource(
  source: MusicSourceId,
  title: string,
  artist: string,
  album: string,
): Promise<SongCandidate[]> {
  return invokeCommand<SongCandidate[]>('search_source', { source, title, artist, album })
}

/** 点选歌词候选拉文本（`MusicSourceId, String → Option<String>`；None = 取词失败，供 C2 换源）。 */
export function fetchLyric(source: MusicSourceId, id: string): Promise<string | null> {
  return invokeCommand<string | null>('fetch_lyric', { source, id })
}

/** 点选封面候选下载缩略图（`String → Vec<u8>`；返回裸 bytes，mime 嗅探/压缩/转 data URL 在前端 lib/cover.ts）。 */
export function downloadCover(url: string): Promise<number[]> {
  return invokeCommand<number[]>('download_cover', { url })
}

/** 分级兜底年份查询（FR-8 #18）：仅由「补全年份」按钮手动触发，离线 / 全源失败不调用。
 *  一级同平台备用端点优先，无可信结果才跑二级（Deezer / MusicBrainz），命中即停。 */
export function searchMetadataFallback(
  title: string,
  artist: string,
  album: string,
): Promise<MetadataCandidate[]> {
  return invokeCommand<MetadataCandidate[]>('search_metadata_fallback', { title, artist, album })
}

/** 独立在线搜词（千千 Plus + Rangotec 两源并发）：只返回歌词候选，不含封面/年份等元信息。
 *  仅由歌词面板「在线搜词」按钮手动触发；不进五源并发、不改 all_failed / 会话离线。 */
export function searchLyrics(title: string, artist: string): Promise<LyricCandidate[]> {
  return invokeCommand<LyricCandidate[]>('search_lyrics', { title, artist })
}

/** 点选在线歌词候选拉正文（后端已清洗署名/版权声明与增强型时间戳）；None = 取词失败。 */
export function fetchOnlineLyric(provider: LyricsProvider, id: string): Promise<string | null> {
  return invokeCommand<string | null>('fetch_online_lyric', { provider, id })
}
