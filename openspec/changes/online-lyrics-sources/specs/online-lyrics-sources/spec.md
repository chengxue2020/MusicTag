# online-lyrics-sources Specification

## Purpose

为歌词面板提供一个独立的「在线搜词」入口，接入千千 Plus 与 Rangotec 两个**纯歌词**源。两源没有封面 / 发行年份 / 流派 / 音轨等元信息，故与主搜索的五源并发链完全隔离；用户手动点选候选后填入歌词，保存 / `.lrc` 侧载复用既有链路。

## ADDED Requirements

### Requirement: 独立在线歌词契约，不进主搜索

系统 SHALL 为两个纯歌词源定义独立的 `LyricsProvider`（`qianqian` | `rangotec`）与 `LyricCandidate` 契约，并提供 `search_lyrics(title, artist)` 与 `fetch_online_lyric(provider, id)` 两个 Tauri command。

- 系统 SHALL NOT 扩展 `MusicSourceId`、实现 `MusicSource` trait 或把两源加入 `search_song` 的并发源列表。
- 在线搜词 SHALL NOT 改写 `source_stats`、`all_failed`、`isOffline`、`lastSearchAllFailed`、C2 换源链或 `lyricCandidates` / `coverCandidates`。

#### Scenario: 主搜索结果不受影响
- **WHEN** 用户点击「在线搜词」并拿到候选
- **THEN** 主搜索的候选区、离线判定与换源行为保持调用前状态

### Requirement: 两源并发查询与失败降级

`search_lyrics` SHALL 并发查询千千 Plus 与 Rangotec，每源 6 秒超时；单源网络 / 解析失败只降级为空结果，另一源结果照常返回。空 title SHALL 直接返回空列表且不发请求。查询词 SHALL 先清洗控制字符与易触发上游过滤的符号。

#### Scenario: 单源失败
- **WHEN** 千千 Plus 超时而 Rangotec 返回候选
- **THEN** 仍返回 Rangotec 的候选，且不置会话离线

#### Scenario: 上游无结果噪声
- **WHEN** 千千 Plus 在无匹配时返回一批不相关热门候选
- **THEN** 标题不相关的候选被本地打分过滤掉，不展示给用户

### Requirement: 编码兼容与业务码判定

系统 SHALL 按 `X-Lyric-Encoding` → `Content-Type charset` → XML 声明 → 严格 UTF-8 探测（失败回退 GBK）的优先级解码千千 Plus 的 XML 与 LRC 文本。Rangotec 的 JSON 响应 SHALL 先判业务码 `code == 200`；HTTP 200 但 `code` 非 200 SHALL 视为空结果。

#### Scenario: GBK 歌词
- **WHEN** 取词响应头带 `X-Lyric-Encoding: gbk`
- **THEN** 中文歌词正确解码，不出现乱码或替换字符

### Requirement: 点选才取词并清洗 LRC 正文

`fetch_online_lyric` SHALL 按 provider + id 惰性拉取 LRC，返回前完成清洗；清洗后为空或请求失败 SHALL 返回 `None`。

- 系统 SHALL 去掉千千 Plus 的 `[by:QianQian.Plus]` 署名以及「本歌词由 QianQian.Plus 提供。」「来源于网络，版权归原作者及版权方所有。」版权声明。
- 系统 SHALL 过滤版权警示 / 翻唱广告行，并把增强型双时间戳 `[00:00:820,00:04:100]` 转成标准 `[00:00.82]`。
- 系统 SHALL 保留标准 `[mm:ss.xx]` 时间轴与有内容的元信息标签，不改写歌词正文文本。

#### Scenario: 千千 Plus 水印
- **WHEN** 点选千千 Plus 候选
- **THEN** 填入歌词区的文本不含 `QianQian.Plus` 署名与版权声明

#### Scenario: 增强型时间戳
- **WHEN** 点选返回 `[00:00:820,00:04:100]歌词` 的候选
- **THEN** 填入的是 `[00:00.82]歌词`

### Requirement: 手动入口、候选生命周期与写盘链路

歌词面板 SHALL 提供「在线搜词」按钮与独立候选区（搜索中 / 取词中 / 候选列表 / 空态 / 错误提示）。在线搜词 SHALL 只由用户手动点击触发，不参与选中即搜。点选候选 SHALL 把清洗后的歌词填入 `current.lyrics`（仍可编辑），来源 badge SHALL 显示在线 provider；候选不落盘、切歌即弃。保存与「同时保存为 .lrc」SHALL 复用既有 `save_song` / 侧载 `.lrc` 链路。

#### Scenario: 点选填词
- **WHEN** 用户点选一条在线歌词候选且取词成功
- **THEN** 歌词输入框填入清洗后的 LRC、dirty 翻转、badge 显示「千千 Plus」或「Rangotec」

#### Scenario: 切歌即弃
- **WHEN** 在线候选还在展示时用户切到另一首歌
- **THEN** 在线候选清空、状态归 idle、在途请求结果被丢弃

### Requirement: Rangotec 候选身份字段清洗、去重与统一展示

Rangotec 为 UGC 歌词库，身份字段含占位字面量、与歌名重复的专辑值及大量仅 id 不同的重复记录。系统 SHALL 在解析与排序阶段清洗并去重候选；两个来源的候选条 SHALL 采用相同的显示格式。

- 系统 SHALL 把 `id` / `title` / `artist` / `album` 中的占位字面量（`None` / `null` / `nil` / `undefined` / `n/a`，忽略大小写与首尾空白）清洗为空串；`id` 或 `title` 清洗后为空的条目 SHALL 丢弃。
- 当 `album` 归一化后等于 `title` 时，系统 SHALL 把 `album` 视为空，避免歌名污染身份去重。
- 对 `LyricsProvider::Rangotec`，系统 SHALL 在按 id 去重之外再按归一化的 `(title, artist, album)` 身份去重，保留上游 `od=asc` 顺序的第一条；不同歌手或专辑的版本 SHALL 各自保留。千千 Plus SHALL 维持只按 id 去重的既有行为。`album` 仍随契约返回并参与身份去重，但不用于展示。
- 在线候选条 SHALL 只显示来源标签与「`title` — `artist`」，SHALL NOT 显示 `album`、匹配度、id 或任何其他字段；`artist` 为空时 SHALL 只显示 `title`（后面留空），不留悬空连接符；tooltip SHALL 与可见文案一致。

#### Scenario: 占位字段与重复歌名专辑
- **WHEN** Rangotec 返回 `artist="None"`、`album="可能"`（与 `title` 相同）以及 `id="None"` 的条目
- **THEN** 候选条不出现 `None` 字面量、歌名或专辑名的重复文字，`id` 为占位值的条目被丢弃

#### Scenario: 同一首歌的大量重复记录
- **WHEN** Rangotec 一次返回 50 条仅 `id` 不同、`title` / `artist` / `album` 完全相同的记录
- **THEN** 候选列表只保留上游顺序的第一条，且不同歌手或专辑的版本仍能各自显示

#### Scenario: 两个来源显示格式一致
- **WHEN** 在线搜词同时返回千千 Plus 与 Rangotec 候选，且 Rangotec 候选带非空 `album`
- **THEN** 两条候选都只显示「来源标签 + 歌名 — 歌手」，都不显示专辑名，且 tooltip 与可见文案相同

#### Scenario: 歌手为空的候选条
- **WHEN** 在线候选的 `artist` 为空
- **THEN** 候选条只显示歌名，后面留空，不显示连接符、专辑列或其他字段
