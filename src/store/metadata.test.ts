// 元数据候选 A / 分级兜底 B 的 store 行为单测（FR-3 #8、FR-8 #16/#17/#18/#19）。
// 覆盖：点选只填当前为空的字段、不覆盖已填、readonly 守卫；
// 「补全年份」联网关（本次 all_failed / 年份已填 / A 已带出可信年份）；
// 兜底搜索的竞态作废、失败降级空态、不标会话离线；切歌即弃。
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('../api/search', () => ({
  searchSongs: vi.fn(async () => ({
    songs: [],
    source_stats: [
      ['netease', 0], ['qqmusic', 0], ['kugou', 0], ['lrclib', 0], ['itunes', 0],
    ],
    all_failed: false,
  })),
  searchSource: vi.fn(async () => []),
  fetchLyric: vi.fn(async () => null),
  downloadCover: vi.fn(async () => []),
  searchMetadataFallback: vi.fn(async () => []),
  searchLyrics: vi.fn(async () => []),
  fetchOnlineLyric: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => undefined),
}))

import type { MetadataCandidate, SearchResult, Song, SongCandidate } from '../api/types'
import {
  applyMetadataCandidate,
  autoSearchOnSelect,
  manualSearch,
  open,
  runFallback,
  songStore,
} from './song'

const makeSong = (over: Partial<Song> = {}): Song => ({
  path: '/a/song.flac',
  title: '歌名',
  artist: '作者',
  album: '专辑',
  album_artist: '',
  track: '',
  track_total: '',
  year: '',
  genre: '',
  lyrics: '',
  lyrics_source: 'none',
  cover: null,
  cover_mime: null,
  ...over,
})

const makeCand = (over: Partial<SongCandidate> = {}): SongCandidate => ({
  source: 'itunes',
  id: 'i1',
  title: '歌名',
  artist: '作者',
  album: '专辑',
  cover_url: null,
  year: '2003',
  genre: 'Pop',
  track: '3',
  track_total: '12',
  ...over,
})

const makeFallbackCand = (over: Partial<MetadataCandidate> = {}): MetadataCandidate => ({
  year: '2003',
  title: '歌名',
  artist: '作者',
  album: '专辑',
  source: 'Deezer',
  score: 1.1,
  ...over,
})

const emptyResult = (songs: SongCandidate[] = [], all_failed = false): SearchResult => ({
  songs,
  source_stats: [
    ['netease', 0], ['qqmusic', 0], ['kugou', 0], ['lrclib', 0], ['itunes', 0],
  ],
  all_failed,
})

/** 每个用例前把 store 复位成「打开一首年份为空的歌、常规搜索成功但无元数据」的干净态。 */
function reset(song: Song = makeSong()): void {
  songStore.current = { ...song }
  songStore.original = { ...song }
  songStore.readonly = false
  songStore.selectedPath = song.path
  songStore.isOffline = false
  songStore.saveState = 'idle'
  songStore.searchedThisSong = false
  songStore.lyricFetchEmpty = false
  songStore.lyricSourcePlatform = null
  songStore.lyricCandidates = []
  songStore.coverCandidates = []
  songStore.lyricSearchState = 'idle'
  songStore.coverSearchState = 'idle'
  songStore.metadataCandidates = []
  songStore.yearFallbackCandidates = []
  songStore.metadataSearchState = 'idle'
  songStore.metadataSearchSeq = 0
  songStore.lastSearchAllFailed = false
}

describe('applyMetadataCandidate — 点选候选只填当前为空的字段（FR-3 #8）', () => {
  beforeEach(() => reset())

  it('A 候选：年份/流派/音轨号+总数全部填入，且 dirty 翻转', () => {
    applyMetadataCandidate(makeCand())
    expect(songStore.current!.year).toBe('2003')
    expect(songStore.current!.genre).toBe('Pop')
    expect(songStore.current!.track).toBe('3')
    expect(songStore.current!.track_total).toBe('12')
    expect(songStore.dirty).toBe(true)
  })

  it('不覆盖用户已填内容，也不自动继承字段间的值', () => {
    reset(makeSong({ year: '1999', genre: 'Rock' }))
    applyMetadataCandidate(makeCand())
    expect(songStore.current!.year).toBe('1999')
    expect(songStore.current!.genre).toBe('Rock')
    expect(songStore.current!.track).toBe('3') // 仍为空 → 可填
    expect(songStore.current!.track_total).toBe('12')
    expect(songStore.current!.album_artist).toBe('') // 不从 artist/album 继承
  })

  it('null / 空串不写入；B 候选只带 year 也能填', () => {
    applyMetadataCandidate(makeCand({ year: null, genre: '', track: null, track_total: '0' }))
    expect(songStore.current!.year).toBe('')
    expect(songStore.current!.genre).toBe('')
    expect(songStore.current!.track).toBe('')
    expect(songStore.current!.track_total).toBe('0')

    applyMetadataCandidate(makeFallbackCand())
    expect(songStore.current!.year).toBe('2003')
    expect(songStore.current!.genre).toBe('') // B 不带 genre，不臆造
  })

  it('readonly（坏标签）/ 无歌 → 不写、不报错', () => {
    songStore.readonly = true
    applyMetadataCandidate(makeCand())
    expect(songStore.current!.year).toBe('')

    songStore.readonly = false
    songStore.current = null
    expect(() => applyMetadataCandidate(makeCand())).not.toThrow()
  })

  it('候选文本原样填入（繁简不折叠）', () => {
    applyMetadataCandidate(makeCand({ genre: '華語流行' }))
    expect(songStore.current!.genre).toBe('華語流行')
  })
})

describe('needYearFallback — 「查不到」的三种状态（FR-8 #17/#18）', () => {
  beforeEach(() => reset())

  it('② 常规搜索成功但候选无可信年份 + 年份为空 → 允许 B', () => {
    expect(songStore.needYearFallback).toBe(true)
  })

  it('年份已填 / 无歌 / readonly → 不触发 B', () => {
    reset(makeSong({ year: '2003' }))
    expect(songStore.needYearFallback).toBe(false)
    songStore.readonly = true
    expect(songStore.needYearFallback).toBe(false)
    reset()
    songStore.current = null
    expect(songStore.needYearFallback).toBe(false)
  })

  it('③ 本次常规搜索 all_failed=true（离线）→ 不触发、不发请求；判据是本次而非「曾离线」', () => {
    songStore.isOffline = false
    songStore.lastSearchAllFailed = true
    expect(songStore.needYearFallback).toBe(false)

    // 会话曾离线但本次手动搜索成功 → 重新放行
    songStore.isOffline = true
    songStore.lastSearchAllFailed = false
    expect(songStore.needYearFallback).toBe(true)
  })

  it('① A 已带出可信年份 → 只展示候选，不触发 B；脏年份不算可信', () => {
    songStore.metadataCandidates = [makeCand()]
    expect(songStore.needYearFallback).toBe(false)

    songStore.metadataCandidates = [makeCand({ year: '1899' })]
    expect(songStore.needYearFallback).toBe(true)
    songStore.metadataCandidates = [makeCand({ year: 'abc' })]
    expect(songStore.needYearFallback).toBe(true)
  })

  it('本首未跑常规搜索（歌词/封面齐全）→ 按钮仍显示，但只是「先补跑 A」的入口', () => {
    songStore.lastSearchAllFailed = null
    songStore.metadataCandidates = []
    expect(songStore.needYearFallback).toBe(true)
  })
})

describe('runFallback — A 未跑时先补跑常规五源（FR-8 #17 ② 前置）', () => {
  beforeEach(() => reset())

  it('A 成功但无可信年份 → 才调用 B，并落库 A 候选与本次 all_failed', async () => {
    songStore.lastSearchAllFailed = null
    const stubA = vi.fn(async () => emptyResult([], false))
    const stubB = vi.fn(async () => [makeFallbackCand()])
    await runFallback(stubB, stubA)
    expect(stubA).toHaveBeenCalledWith('歌名', '作者', '专辑')
    expect(stubB).toHaveBeenCalledWith('歌名', '作者', '专辑')
    expect(songStore.lastSearchAllFailed).toBe(false)
    expect(songStore.yearFallbackCandidates).toEqual([makeFallbackCand()])
    expect(songStore.metadataSearchState).toBe('done')
  })

  it('A 已带出可信年份（①）→ 不调用 B，只展示 A 候选', async () => {
    songStore.lastSearchAllFailed = null
    const a = makeCand()
    const stubB = vi.fn(async () => [makeFallbackCand()])
    await runFallback(stubB, async () => emptyResult([a], false))
    expect(stubB).not.toHaveBeenCalled()
    expect(songStore.metadataCandidates).toEqual([a])
    expect(songStore.metadataSearchState).toBe('idle')
    expect(songStore.needYearFallback).toBe(false)
  })

  it('A 全源失败（③）→ 不调用 B、不发备用请求，按钮隐藏', async () => {
    songStore.lastSearchAllFailed = null
    const stubB = vi.fn(async () => [makeFallbackCand()])
    await runFallback(stubB, async () => emptyResult([], true))
    expect(stubB).not.toHaveBeenCalled()
    expect(songStore.lastSearchAllFailed).toBe(true)
    expect(songStore.needYearFallback).toBe(false)
    expect(songStore.metadataSearchState).toBe('idle')
  })

  it('A 抛错 → 不调用 B、不标会话离线，按钮保留可重试', async () => {
    songStore.lastSearchAllFailed = null
    const stubB = vi.fn(async () => [makeFallbackCand()])
    await runFallback(stubB, async () => { throw new Error('IPC 失败') })
    expect(stubB).not.toHaveBeenCalled()
    expect(songStore.lastSearchAllFailed).toBeNull()
    expect(songStore.isOffline).toBe(false)
    expect(songStore.needYearFallback).toBe(true)
    expect(songStore.metadataSearchState).toBe('idle')
  })
})

describe('runFallback — 手动兜底查询（FR-8 #18/#19）', () => {
  beforeEach(() => reset())

  it('注入桩返回候选 → 存入 yearFallbackCandidates、state=done、参数为本歌标签', async () => {
    const list = [makeFallbackCand()]
    const stub = vi.fn(async () => list)
    await runFallback(stub)
    expect(stub).toHaveBeenCalledWith('歌名', '作者', '专辑')
    expect(songStore.yearFallbackCandidates).toEqual(list)
    expect(songStore.metadataSearchState).toBe('done')
  })

  it('离线 / 年份已填 / readonly → 不发任何请求', async () => {
    const stub = vi.fn(async () => [makeFallbackCand()])
    songStore.lastSearchAllFailed = true
    await runFallback(stub)
    expect(stub).not.toHaveBeenCalled()

    reset(makeSong({ year: '2003' }))
    await runFallback(stub)
    expect(stub).not.toHaveBeenCalled()

    reset()
    songStore.readonly = true
    await runFallback(stub)
    expect(stub).not.toHaveBeenCalled()
  })

  it('请求失败 → 降级空态，不标会话离线（手动兜底不影响常规搜索状态）', async () => {
    const stub = vi.fn(async () => { throw new Error('网络失败') })
    await runFallback(stub)
    expect(songStore.yearFallbackCandidates).toEqual([])
    expect(songStore.metadataSearchState).toBe('done')
    expect(songStore.isOffline).toBe(false)
  })

  it('切歌 / 被新请求作废（序号变化）→ 过期结果丢弃', async () => {
    const stub = vi.fn(async () => {
      songStore.metadataSearchSeq++ // 模拟切歌 resetSearchState 的序号自增
      return [makeFallbackCand()]
    })
    await runFallback(stub)
    expect(songStore.yearFallbackCandidates).toEqual([])
  })

  it('连点防并发：searching 期间第二次调用直接返回', async () => {
    let finish!: (v: MetadataCandidate[]) => void
    const pending = new Promise<MetadataCandidate[]>((resolve) => { finish = resolve })
    const first = runFallback(() => pending)
    const secondStub = vi.fn(async () => [makeFallbackCand()])
    await runFallback(secondStub)
    expect(secondStub).not.toHaveBeenCalled()
    finish([])
    await first
    expect(songStore.metadataSearchState).toBe('done')
  })
})

describe('常规搜索结果喂 A 候选 + 本次 all_failed 判据（FR-8 #16/#17）', () => {
  beforeEach(() => reset())

  it('autoSearchOnSelect：带元数据的候选进 metadataCandidates，无字段的被过滤', async () => {
    const withMeta = makeCand()
    const withoutMeta = makeCand({ id: 'n1', source: 'netease', year: null, genre: null, track: null, track_total: null })
    await autoSearchOnSelect(async () => emptyResult([withMeta, withoutMeta], false))
    expect(songStore.metadataCandidates).toEqual([withMeta])
    expect(songStore.lastSearchAllFailed).toBe(false)
  })

  it('autoSearchOnSelect：all_failed=true → 本次判据为 true、会话离线、A 候选清空', async () => {
    songStore.metadataCandidates = [makeCand()]
    await autoSearchOnSelect(async () => emptyResult([], true))
    expect(songStore.lastSearchAllFailed).toBe(true)
    expect(songStore.isOffline).toBe(true)
    expect(songStore.needYearFallback).toBe(false)
  })

  it('autoSearchOnSelect：IPC reject → lastSearchAllFailed=true、A 候选清空', async () => {
    songStore.metadataCandidates = [makeCand()]
    await autoSearchOnSelect(async () => { throw new Error('IPC 失败') })
    expect(songStore.lastSearchAllFailed).toBe(true)
    expect(songStore.metadataCandidates).toEqual([])
  })

  it('manualSearch 成功可把「曾离线」的会话重新放行（判据是本次结果）', async () => {
    songStore.isOffline = true
    songStore.lastSearchAllFailed = true
    await manualSearch('lyrics', async () => emptyResult([], false))
    expect(songStore.lastSearchAllFailed).toBe(false)
    expect(songStore.needYearFallback).toBe(true)
    expect(songStore.isOffline).toBe(true) // D3：手动搜索不改写会话离线态
  })

  it('切歌（open 成功）→ A/B 候选清空、兜底态归 idle、序号自增作废在途请求', async () => {
    songStore.metadataCandidates = [makeCand()]
    songStore.yearFallbackCandidates = [makeFallbackCand()]
    songStore.metadataSearchState = 'done'
    songStore.metadataSearchSeq = 7
    songStore.lastSearchAllFailed = true
    songStore.selectedPath = '/a/next.flac' // open() 的过期响应守卫要求选中路径已切到新歌
    await open('/a/next.flac', async () => makeSong({ path: '/a/next.flac', title: 'Next' }))
    expect(songStore.metadataCandidates).toEqual([])
    expect(songStore.yearFallbackCandidates).toEqual([])
    expect(songStore.metadataSearchState).toBe('idle')
    expect(songStore.metadataSearchSeq).toBe(8)
    expect(songStore.lastSearchAllFailed).toBeNull() // 本首尚未跑常规搜索（三态显式）
  })
})
