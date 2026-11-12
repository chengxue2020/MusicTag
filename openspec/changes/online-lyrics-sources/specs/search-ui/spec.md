# search-ui Specification Delta

## MODIFIED Requirements

### Requirement: 手动搜索按钮（歌词区）

歌词区 SHALL 提供两个互不干扰的手动按钮：「🔍 搜索歌词」触发既有五源主搜索（`manualSearch('lyrics')`），「🌐 在线搜词」触发独立在线歌词查询（`searchOnlineLyrics()`）。两者 SHALL 使用各自独立的状态机、候选区与过期序号。

- 「在线搜词」SHALL NOT 触发 `searchSongs`、SHALL NOT 改写 `lyricSearchState` / `lyricCandidates` / `lyricFetchEmpty` / `isOffline` / `lastSearchAllFailed`。
- 坏标签只读（readonly）或无选中歌曲时，两个按钮 SHALL 均禁用。
- 在线搜词进行中（搜索候选或点选后取正文）SHALL 禁用「在线搜词」按钮并显示「搜索中… / 取词中…」。

#### Scenario: 两个按钮互不干扰
- **WHEN** 用户点击「在线搜词」并拿到在线候选
- **THEN** 主搜索候选区（含折叠按钮、C2 空态、离线提示）保持原有状态，不出现「搜索中…」

#### Scenario: 在线候选渲染
- **WHEN** 在线搜词返回候选
- **THEN** 独立候选区按 `千千 Plus / Rangotec` 来源标签统一展示「歌名 — 作者」（无作者则后面留空），不展示专辑等其他信息，点选后才拉取正文

#### Scenario: 只读守卫
- **WHEN** 当前歌曲为坏标签只读
- **THEN** 「搜索歌词」与「在线搜词」均禁用，点击不发任何请求
