## Why

用户提供了两个纯歌词接口：千千静听歌词服务器（`https://lyrics.qianqian.plus/api/search/`）与 Rangotec 歌词服务（`http://yun.rangotec.com:86/qianqian` / `/desc` 及作者公开的 JSON API）。实测两者**只返回歌词**，没有标题以外的完整元信息（无封面、发行年份、流派、音轨），不适合塞进现有的五源并发 `search_song` 聚合链——那会污染 `MusicSourceId` 来源序、`source_stats`、`all_failed` 与 C2 换源语义。

用户明确拍板：**「把我刚给你的接口做成歌词接口就可以了，不需要放到主搜索接口里（因为没有其他的元信息）」**，并且要求**去掉千千 Plus 的署名 / 版权声明**。因此做成歌词面板里一个独立的「在线搜词」入口，两个源都接。

## What Changes

- **新增独立在线歌词链路**：Rust `service/lyrics_online.rs` 实现两个 provider（千千 Plus / Rangotec），新增 `search_lyrics` / `fetch_online_lyric` 两个 Tauri command 并注册；契约类型 `LyricsProvider` / `LyricCandidate` 与 `MusicSourceId` / `SongCandidate` 完全分离。
- **不碰主搜索**：`searcher::search_song` 五源列表、`source_stats`、`all_failed`、C2 换源顺序、`autoSearchOnSelect` / `manualSearch` / `pickLyricCandidate` 一律不改；在线搜词有自己的状态机与过期序号。
- **歌词清洗**：按响应头 / XML 声明用 `encoding_rs` 解码 UTF-8 / GBK（Cargo.toml 显式声明依赖）；去掉千千 Plus `[by:QianQian.Plus]` 署名与「本歌词由…提供。」「来源于网络，版权归…」版权声明；滤掉 Rangotec 的版权警示 / 翻唱广告行；增强型双时间戳 `[00:00:820,00:04:100]` 转标准 `[00:00.82]`；去空元信息标签行。
- **前端**：`src/api/types.ts` / `api/search.ts` 增契约与封装；`store/song.ts` 增 `searchOnlineLyrics` / `pickOnlineLyric`；`LyricPanel.vue` 增「🌐 在线搜词」按钮与独立候选区；新增 `OnlineLyricCandidate.vue`；来源 badge 新增在线 provider 文案。点选后填入 `current.lyrics`，保存 / `.lrc` 侧载仍走既有链路。
- **文档与合规**：README 功能与数据披露、合规记录新增两个源专节、design.md §10.3 与 V1-PRD.md §7 / openspec/config.yaml 契约清单同步（23 → 25 个 command），契约守卫测试数量同步。

## Capabilities

### New Capabilities
- `online-lyrics-sources`: 手动「在线搜词」——两个纯歌词源并发查询、候选点选填词、独立状态机、LRC 清洗与编码兼容；不进主搜索。

### Modified Capabilities
- `search-ui`: 歌词面板新增第二个手动按钮与独立候选区；明确「在线搜词」不参与选中即搜、离线判定与 C2 换源，既有主搜索交互语义不变。

## Impact

- Rust：`src-tauri/src/service/lyrics_online.rs`（新）、`service/mod.rs`、`model.rs`、`commands/search.rs`、`lib.rs`、`Cargo.toml`。
- 前端：`src/api/types.ts`、`src/api/search.ts`、`src/store/selectors.ts`、`src/store/song.ts`、`src/components/LyricPanel.vue`、`src/components/OnlineLyricCandidate.vue`（新）。
- 测试：`src-tauri/tests/lyrics_online_tests.rs`（新）、`src/api/search.test.ts`、`src/store/online-lyrics.test.ts`（新）、`src/components/lyric-panel.test.ts`、`既有 5 处 api/search mock 补桩`、`src/styles/command-contract.test.ts`。
- 文档：`README.md`、`docs/compliance/search-sources.md`、`docs/design/design.md`、`docs/V1-PRD.md`、`openspec/config.yaml`。
