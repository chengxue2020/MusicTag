// 年份 / 元数据区组件单测（FR-3 #8、FR-8 #16/#17/#18/#19）。
// A 候选列表 / 点选填入、「补全年份」按钮三态显隐、B 候选列表 + 来源与匹配度、
// 搜索中 / 空态 / 离线提示。沿用 lyric-panel.test.ts 的 api/search + core mock 先例。
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

const { mockFallback } = vi.hoisted(() => ({
  mockFallback: vi.fn(async () => [] as Array<{
    year: string; title: string; artist: string; album: string; source: string; score: number
  }>),
}))

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
  searchMetadataFallback: mockFallback,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => undefined),
}))

import type { MetadataCandidate, Song, SongCandidate } from '../api/types'
import { searchSongs } from '../api/search'
import { songStore } from '../store/song'
import MetadataPanel from './MetadataPanel.vue'

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

const makeB = (over: Partial<MetadataCandidate> = {}): MetadataCandidate => ({
  year: '2003',
  title: '歌名',
  artist: '作者',
  album: '专辑',
  source: 'Deezer',
  score: 1.1,
  ...over,
})

function reset(song: Song = makeSong()): void {
  songStore.current = { ...song }
  songStore.original = { ...song }
  songStore.readonly = false
  songStore.selectedPath = song.path
  songStore.isOffline = false
  songStore.metadataCandidates = []
  songStore.yearFallbackCandidates = []
  songStore.metadataSearchState = 'idle'
  songStore.metadataSearchSeq = 0
  songStore.lastSearchAllFailed = false
  mockFallback.mockReset()
  mockFallback.mockResolvedValue([])
  vi.mocked(searchSongs).mockClear()
}

describe('MetadataPanel — A 候选（FR-8 #16）', () => {
  beforeEach(() => reset())

  it('无 A 候选时不渲染候选行', () => {
    const w = mount(MetadataPanel)
    expect(w.find('[data-testid="metadata-panel"]').exists()).toBe(true)
    expect(w.findAll('.cand-row')).toHaveLength(0)
  })

  it('A 候选显示来源、年份/流派/音轨与歌名，点选后只填当前为空的字段', async () => {
    songStore.metadataCandidates = [makeCand()]
    const w = mount(MetadataPanel)
    expect(w.findAll('.cand-row')).toHaveLength(1)
    expect(w.text()).toContain('iTunes')
    expect(w.text()).toContain('2003 · Pop · 音轨 3/12')
    expect(w.text()).toContain('歌名 — 作者')

    songStore.current!.genre = '我填的流派'
    await w.find('.cand-row').trigger('click')
    expect(songStore.current!.year).toBe('2003')
    expect(songStore.current!.track).toBe('3')
    expect(songStore.current!.track_total).toBe('12')
    expect(songStore.current!.genre).toBe('我填的流派') // 不覆盖
  })
})

describe('MetadataPanel — 「补全年份」联网关（FR-8 #17/#18）', () => {
  beforeEach(() => reset())

  it('年份为空、本次搜索成功、A 无可信年份 → 显示按钮', () => {
    const w = mount(MetadataPanel)
    expect(w.find('[data-testid="year-fallback-trigger"]').exists()).toBe(true)
  })

  it('年份已填 / 本次 all_failed=true / A 已带出可信年份 → 按钮不显示', () => {
    reset(makeSong({ year: '2003' }))
    expect(mount(MetadataPanel).find('[data-testid="year-fallback-trigger"]').exists()).toBe(false)

    reset()
    songStore.lastSearchAllFailed = true
    expect(mount(MetadataPanel).find('[data-testid="year-fallback-trigger"]').exists()).toBe(false)

    reset()
    songStore.metadataCandidates = [makeCand()]
    expect(mount(MetadataPanel).find('[data-testid="year-fallback-trigger"]').exists()).toBe(false)
  })

  it('readonly（坏标签）→ 按钮不显示', () => {
    songStore.readonly = true
    expect(mount(MetadataPanel).find('[data-testid="year-fallback-trigger"]').exists()).toBe(false)
  })

  it('点击按钮 → 发起兜底查询（后台异步），完成后渲染带来源与匹配度的 B 候选', async () => {
    mockFallback.mockResolvedValue([makeB(), makeB({ source: 'MusicBrainz', score: 0.86, year: '2004' })])
    const w = mount(MetadataPanel)
    await w.find('[data-testid="year-fallback-trigger"]').trigger('click')
    await flushPromises()
    expect(mockFallback).toHaveBeenCalledWith('歌名', '作者', '专辑')
    expect(w.findAll('.cand-row')).toHaveLength(2)
    expect(w.text()).toContain('Deezer')
    expect(w.text()).toContain('MusicBrainz')
    expect(w.text()).toContain('匹配 1.10')
  })

  it('本首未跑常规搜索 → 显示说明，点击先查常规来源、仍无可信年份再查备用来源', async () => {
    songStore.lastSearchAllFailed = null
    mockFallback.mockResolvedValue([makeB()])
    const w = mount(MetadataPanel)
    expect(w.find('.fallback-note').exists()).toBe(true)
    expect(w.find('[data-testid="year-fallback-trigger"]').exists()).toBe(true)

    await w.find('[data-testid="year-fallback-trigger"]').trigger('click')
    await flushPromises()
    expect(vi.mocked(searchSongs)).toHaveBeenCalledWith('歌名', '作者', '专辑')
    expect(mockFallback).toHaveBeenCalledWith('歌名', '作者', '专辑')
    expect(w.findAll('.cand-row')).toHaveLength(1)
  })

  it('B 候选点选 → 只填年份，不臆造流派/音轨', async () => {
    songStore.yearFallbackCandidates = [makeB()]
    songStore.metadataSearchState = 'done'
    const w = mount(MetadataPanel)
    await w.find('.cand-row').trigger('click')
    expect(songStore.current!.year).toBe('2003')
    expect(songStore.current!.genre).toBe('')
    expect(songStore.current!.track).toBe('')
  })

  it('搜索中显示状态并禁用按钮；无结果显示空态；离线显示手动填写提示', () => {
    songStore.metadataSearchState = 'searching'
    const searching = mount(MetadataPanel)
    expect(searching.text()).toContain('正在查询年份…')
    expect(searching.find('[data-testid="year-fallback-trigger"]').attributes('disabled')).toBeDefined()

    songStore.metadataSearchState = 'done'
    songStore.yearFallbackCandidates = []
    expect(mount(MetadataPanel).text()).toContain('备用来源也未找到可信年份，可手动填写')

    reset()
    songStore.lastSearchAllFailed = true
    expect(mount(MetadataPanel).text()).toContain('离线：仅手动填写年份')
  })
})
