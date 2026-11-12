# 设计：独立在线歌词接口（online-lyrics-sources）

## D1 不进主搜索（产品约束，最高优先级）

两个源只有歌词，没有封面 / 年份 / 流派 / 音轨。若注册为第 6 个 `MusicSource`：

- `SongCandidate` 会被迫伪造空 `cover_url` / `year`；
- `source_stats` 多一项、`all_failed` 的「五源全失败」语义被改写；
- C2 换源链会把它们当成可出候选的源，破坏既有身份校验。

因此**作废**早期「第 6 个搜索源」方案：不扩 `MusicSourceId`、不实现 `MusicSource` trait、不进 `search_song_with_sources`。新增 `LyricsProvider`（`qianqian` / `rangotec`）与 `LyricCandidate` 独立契约，走 `search_lyrics` / `fetch_online_lyric` 两个 command。

## D2 协议与编码

| 源 | 搜索 | 取词 |
|---|---|---|
| 千千 Plus | `GET /api/search/?sh&Title=<...>&Artist=<...>` → XML `<result><lrc id artist title/></result>` | `GET /api/search/?dl&Id=<id>` → 纯文本 LRC，响应头 `X-Lyric-Encoding: utf-8\|gbk` |
| Rangotec | `GET https://tools.rangotec.com/api/anon/lrc?title=&artist=&od=asc` → JSON `{code,msg,data:[{id,artist,title,album,lrc,...}]}` | `GET /qianqian?dl?Id=<id>&Code=abc`（老协议下载动作，实测稳定） |

- 千千 Plus 的 action 是**无值裸参数**（`?sh&Title=...`），`Url::parse_with_params` 会退化成 `sh=`，服务端不认 → 手工拼串 + 自实现的 UTF-8 百分号编码 `pct_encode`。
- 编码混合：中文参数常 `charset=UTF-8`，ASCII 参数可能 `charset=GBK` / XML 声明 `GB2312`，LRC 正文混用 UTF-8 与 GBK → 解码优先级 `X-Lyric-Encoding` → `Content-Type charset` → XML 声明 → 严格 UTF-8 探测，失败才 GBK。用 `encoding_rs`（Cargo.lock 已间接含 0.8.35，Cargo.toml 显式声明）。
- Rangotec **HTTP 200 但 `code=400`** 是无结果 → 先判业务码。`od=asc` 取老数据（标准 LRC）；`od=desc` / 默认优先最新 UGC，多为增强型双时间戳且夹带版权 / 翻唱广告 → 默认只用 asc。
- `/qianqian/desc` 不是新数据源，只是同一服务的结果倒序入口；排序只影响搜索候选，不影响下载。接入时用 JSON API 的 `od=asc|desc`，不分别接两个 URL。
- 老协议 `sh` 搜索对外不可用（GBK 参数返回 `EX4399`、常规参数返回空 XML），故搜索只走公开 JSON API。
- 上游限制：单 IP 1 秒 100 次、凌晨 1–5 点可能关服务、特殊字符过滤严格 → `sanitize_query` 先清洗查询词；两源各自 6s 超时，单源失败只降级空结果。

## D3 候选过滤与排序

千千 Plus 查无结果时仍返回一堆不相关热门候选（上游空结果语义不可用）。复用主搜索的 `searcher::norm`（繁→简折叠 + 全角半角 + 小写，只作用比较侧）与 `title_match` / `artist_match` 权重打分：**标题不相关的候选直接丢弃**，同 provider 按 id 去重，按分数降序取前 8；两源合计 ≤ 16 条，provider 固定序（千千 Plus → Rangotec）。

**Rangotec UGC 身份清洗与去重（v0.2.4 显示修复）**：实测其身份字段存在四类脏数据 —— `artist` / `album` / `id` 落成字符串 `"None"` / `"null"` 等占位值；`album` 直接填成歌名，导致专辑列重复显示歌名；`artist` 为空；一次返回最多 50 条仅 `id` 不同、`(title, artist, album)` 完全相同的记录。处理顺序：

1. `parse_rangotec_search` 用 `clean_provider_field()` 把 `id` / `title` / `artist` / `album` 的占位字面量（忽略大小写与首尾空白）清洗为空；`id` 或 `title` 清洗后为空的条目丢弃；`album` 归一化后等于 `title` 时置空。
2. `rank_candidates` 仅对 `LyricsProvider::Rangotec` 追加按归一化 `(title, artist, album)` 的身份去重，保留上游 `od=asc` 顺序的第一条；不同歌手 / 专辑的版本各自保留。千千 Plus 候选本身带区分度，维持只按 id 去重的既有行为，避免把同歌名的不同版本误合并。
3. 前端 `OnlineLyricCandidate.vue` 在 `artist` 为空时只显示 `title`，不拼接悬空的 ` — `；tooltip 与可见文案保持一致。

## D4 惰性拉取与 LRC 清洗

Rangotec 的 JSON 响应虽含 `lrc` 全文，但按仓库既有「点选才取正文」约定不随 IPC 带出，`search_lyrics` 只返回身份 + 匹配度；点选后 `fetch_online_lyric(provider, id)` 按 id 取词。取回后两源共用 `clean_lrc`：

1. `\r\n` / `\r` → `\n`；
2. 删空元信息标签行 `[ti:]` `[ar:]` `[al:]` `[by:]`；
3. 删含 `QianQian.Plus` / `本歌词由` / `来源于网络` / `版权归原作者` / `未经著作权人书面许可` / `本歌曲翻唱由` / `提供生产能力` / `Vemus` 的行 —— **用户明确要求去掉千千 Plus 署名 / 版权声明**；
4. 增强型双时间戳 `[mm:ss:fff,mm:ss:fff]` → `[mm:ss.xx]`（取第一个时间戳，毫秒四舍五入到厘秒）；标准 `[mm:ss.xx]` 与 `[ti:xxx]` 原样保留；
5. 去掉空行（含删除水印后残留的孤立空行）、以单个换行结尾；清洗后为空 → `None`。

剥离署名只是显示层清洗，**不改变歌词本身的版权归属**（合规文档已注明）。

## D5 前端状态机隔离

`store/song.ts` 新增独立字段：`onlineLyricState` / `onlineLyricCandidates` / `onlineLyricSeq` / `onlineLyricFetching` / `onlineLyricError` / `onlineLyricProvider`。`resetSearchState()` 顺带清在线状态（候选生命周期 = 当前歌曲），但**不改** `lyricSearchState` / `coverSearchState` / `isOffline` / `lastSearchAllFailed` / C2。`onlineLyricSeq` 作废切歌与重搜的在途响应；点选取词用 `onlineLyricFetching` 防并发。

badge 优先级：`onlineLyricProvider` → `lyricSourcePlatform` → `lyricsSource`。在线源不属于 `MusicSourceId`，故不写 `lyricSourcePlatform`（避免污染 C2 平台语义）。`undo()` 一并清 `onlineLyricProvider`。

点选成功只写 `current.lyrics`（在 `DIRTY_FIELDS` 内 → dirty 自动翻转）；保存与「同时保存为 .lrc」完全复用 `save()` / `save_song` / `lyrics::export_lrc`。

## D6 契约同步

`lib.rs` 注册 23 → 25 个 command；`docs/design/design.md` §10.3、`docs/V1-PRD.md` §7、`openspec/config.yaml` 三处清单与 `src/styles/command-contract.test.ts` 的数量断言必须同步，否则契约守卫红。

## D7 验证策略

本机无 Rust 工具链（`cargo: command not found`，全盘亦无 cargo/rustc）→ Rust 侧靠 CI `cargo check/test --all-targets`；前端 `npm run test` / `npm run build` 本机可跑。Rust 测试外置 `src-tauri/tests/lyrics_online_tests.rs`，用 `tests/common` 的 mock HTTP server 注入 base URL，覆盖解析、编码、清洗、业务码与 HTTP 形状，不依赖外网。

本机已验证：`npm run test` 33 文件 / 596 用例全绿（其中新增 16 例覆盖在线搜词链路），`npm run build`（`vue-tsc --noEmit` + `vite build`）通过；生产端点另用真实响应核对过业务码 `code=200`、千千 Plus 水印行、Rangotec 增强型双时间戳与 `X-Lyric-Encoding` 响应头。Rust 侧为静态审查（已对齐仓库既有 `parse_with_params` / `tests/common` 写法），**编译与集成测试须以 CI 结果为准**。
