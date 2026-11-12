# 任务（变更域 rust + frontend + docs）

> 产品约束：两个源只做成独立歌词接口，**不放进主搜索接口**；千千 Plus 的署名 / 版权声明**去掉**。

## G1 Rust 在线歌词服务与独立 IPC

- [x] 1.1 `Cargo.toml` 显式声明 `encoding_rs = "0.8"`（Cargo.lock 已间接含 0.8.35）。
- [x] 1.2 `model.rs` 增 `LyricsProvider` / `LyricCandidate` 契约；明确不扩 `MusicSourceId`、不进 `SearchResult`。
- [x] 1.3 `service/lyrics_online.rs`：两 provider 搜索与取词、`pct_encode` / `sanitize_query` / `decode_bytes`、轻量 XML 解析与实体解码、Rangotec 业务码判定、标题相关性打分过滤、LRC 清洗（去千千 Plus 署名 / 版权声明、增强型双时间戳转标准、滤广告行）。
- [x] 1.4 `service/mod.rs` 声明模块；`commands/search.rs` 增 `search_lyrics` / `fetch_online_lyric`；`lib.rs` 注册两个 command。主搜索链零改动。

## G2 前端独立在线搜词入口

- [x] 2.1 `api/types.ts` 增 `LyricsProvider` / `LyricCandidate`；`api/search.ts` 增 `searchLyrics` / `fetchOnlineLyric` 透传封装。
- [x] 2.2 `store/selectors.ts` 增 `lyricsProviderLabel`；`store/song.ts` 增独立状态机字段、`resetSearchState` 清理、`undo` 清 badge、`searchOnlineLyrics` / `pickOnlineLyric` 动作。不动主搜索 / C2。
- [x] 2.3 `components/OnlineLyricCandidate.vue`（新）+ `LyricPanel.vue`：第二个手动按钮「🌐 在线搜词」、独立候选区（搜索中 / 取词中 / 候选列表 / 空态 / 错误）、badge 优先显示在线来源。
- [x] 2.4 既有 5 处 `vi.mock('../api/search')` 补 `searchLyrics` / `fetchOnlineLyric` 桩（否则 store 默认注入为 undefined）。

## G3 测试与本地验证

- [x] 3.1 `src-tauri/tests/lyrics_online_tests.rs`（新）：解析、编码、清洗、业务码、URL 形状、打分过滤、单源失败降级的 Rust 集成测试（走 `tests/common` mock HTTP server，不依赖外网）。
- [x] 3.2 前端测试：`src/api/search.test.ts` 补两个 command 透传；`src/store/song.test.ts` 末段新增「独立在线搜词」describe（状态机 / 竞态 / 点选填词 / 不进主搜索 / 并发守卫）；`src/components/lyric-panel.test.ts` 新增在线入口 describe 并在 `openSong()` 复位在线状态；5 处既有 `vi.mock('../api/search')` 补桩。
- [x] 3.3 前端本机验证：`npm run test` → 33 文件 / 596 用例全绿（新增 16 例）；`npm run build`（`vue-tsc --noEmit` + `vite build`）通过。
- [ ] 3.4 Rust 侧本机无工具链（`cargo: command not found`，全盘无 cargo/rustc），须提交后由 CI `cargo check/test --all-targets` 验证 `lyrics_online.rs` 与 `lyrics_online_tests.rs` 的编译与行为。

## G4 文档与契约同步

- [x] 4.1 `docs/design/design.md` §10.3 + `docs/V1-PRD.md` §7 + `openspec/config.yaml` 契约清单加两个 command（23 → 25）；`src/styles/command-contract.test.ts` 数量断言同步。
- [x] 4.2 `README.md` 功能特性 + 外部 API 数据披露（域名 / 发送字段 / 触发时机）；`docs/compliance/search-sources.md` 新增两源专节（协议、编码、限流、清洗、版权提示、未决事项引用）。
- [x] 4.3 本 openspec change：`proposal.md` / `design.md` / `tasks.md` + `specs/online-lyrics-sources/spec.md` + `specs/search-ui/spec.md` delta。

## G5 Rangotec 候选显示修复（v0.2.4 重发）

> 用户反馈：千千 Plus 候选显示正确，Rangotec 显示「像歌名但不确定」。实测为上游 UGC 身份字段脏数据 + 大量重复记录，并非编码乱码。

- [x] 5.1 `service/lyrics_online.rs` 增 `clean_provider_field()`：清洗 `id` / `title` / `artist` / `album` 的 `"None"` / `"null"` / `"nil"` / `"undefined"` / `"n/a"` 占位值；`album` 归一化等于 `title` 时置空。
- [x] 5.2 `rank_candidates()` 仅对 Rangotec 追加按归一化 `(title, artist, album)` 的身份去重，保留 `od=asc` 顺序第一条；千千 Plus 维持只按 id 去重。
- [x] 5.3 `components/OnlineLyricCandidate.vue`：artist 为空时只显示歌名，不拼接悬空 ` — `，tooltip 与可见文案一致。
- [x] 5.4 回归测试：`src-tauri/tests/lyrics_online_tests.rs` 增占位字段清洗 / 身份去重 / 千千 Plus 不受影响三例；`src/components/lyric-panel.test.ts` 增候选条渲染一例。
- [x] 5.5 同步 `design.md` D3、`spec.md` 新增 Requirement、`docs/compliance/search-sources.md` Rangotec 已知限制栏、`README.md` v0.2.4 更新说明。
