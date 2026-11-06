<script setup lang="ts">
// 年份 / 元数据区（FR-3 #8、FR-8 #16/#17/#18/#19）：
// - A：常规五源候选里带文本元数据的子集，零新增请求；点选后只填当前为空的字段；
// - B：仅当本次常规搜索成功（all_failed=false）、候选无可信年份、当前年份为空且非只读时，
//   才显示「补全年份」按钮，用户手动点击才查备用来源；离线（all_failed=true）不触发。
//   本首未跑常规搜索（lastSearchAllFailed=null）时按钮也显示，但 store 会先补跑 A，
//   A 成功且仍无可信年份才真正进 B（严格守住 FR-8 #17 ②，不让齐全标签的歌拿不到年份）。
// - 候选生命周期 = 当前歌曲（切歌即弃、不落盘）；结果带来源 + 匹配度，仍须手动点选。
// 分层：组件不直呼 invoke，IPC 全走 store → api/search.ts（layering 守卫）。
import { sourceLabel } from '../store/selectors'
import { applyMetadataCandidate, runFallback, songStore } from '../store/song'

/** A 候选的展示摘要：年份 / 流派 / 音轨号，非空的才显示。 */
function summary(c: { year?: string | null; genre?: string | null; track?: string | null; track_total?: string | null }): string {
  const parts: string[] = []
  if (c.year) parts.push(c.year)
  if (c.genre) parts.push(c.genre)
  if (c.track) parts.push(`音轨 ${c.track_total ? `${c.track}/${c.track_total}` : c.track}`)
  return parts.join(' · ') || '—'
}

/** B 候选匹配度（后端 f32，展示两位小数）。 */
function scoreText(score: number): string {
  return Number.isFinite(score) ? score.toFixed(2) : '—'
}
</script>

<template>
  <section class="metadata" data-testid="metadata-panel">
    <div class="metadata-head">
      <span class="label">年份 / 元数据</span>
      <span class="hint">点选候选才填入，不覆盖已填内容</span>
    </div>

    <!-- A：常规搜索候选带出的元数据 -->
    <div v-if="songStore.metadataCandidates.length" class="cand-list">
      <button
        v-for="c in songStore.metadataCandidates"
        :key="`a:${c.source}:${c.id}`"
        type="button"
        class="cand-row"
        :title="`${c.title} — ${c.artist}`"
        @click="applyMetadataCandidate(c)"
      >
        <span class="src-tag">{{ sourceLabel(c.source) }}</span>
        <span class="cand-values">{{ summary(c) }}</span>
        <span class="cand-text">{{ c.title }} — {{ c.artist }}</span>
      </button>
    </div>

    <!-- B：手动触发的分级兜底查询 -->
    <button
      v-if="songStore.needYearFallback"
      class="fallback-trigger"
      type="button"
      data-testid="year-fallback-trigger"
      :disabled="songStore.metadataSearchState === 'searching'"
      @click="runFallback()"
    >🔍 补全年份</button>
    <div v-if="songStore.needYearFallback && songStore.lastSearchAllFailed === null" class="fallback-note">
      本首尚未执行常规搜索：点击后先查常规来源，查不到年份再查备用来源
    </div>

    <div v-if="songStore.metadataSearchState === 'searching'" class="cand-status">
      正在查询年份…<span class="spinner" aria-hidden="true"></span>
    </div>
    <template v-else-if="songStore.metadataSearchState === 'done'">
      <div v-if="songStore.yearFallbackCandidates.length" class="cand-list">
        <button
          v-for="(c, i) in songStore.yearFallbackCandidates"
          :key="`b:${c.source}:${c.year}:${i}`"
          type="button"
          class="cand-row"
          :title="`${c.title} — ${c.artist}`"
          @click="applyMetadataCandidate(c)"
        >
          <span class="src-tag">{{ c.source }}</span>
          <span class="cand-values">{{ c.year }}</span>
          <span class="cand-text">{{ c.title }} — {{ c.artist }}</span>
          <span class="score">匹配 {{ scoreText(c.score) }}</span>
        </button>
      </div>
      <div v-else class="cand-empty">备用来源也未找到可信年份，可手动填写</div>
    </template>
    <div
      v-else-if="songStore.lastSearchAllFailed === true && songStore.current !== null && !songStore.current.year"
      class="cand-empty"
    >离线：仅手动填写年份</div>
  </section>
</template>

<style scoped>
.metadata {
  border-top: 1px solid var(--border);
  padding-top: 16px;
}

.metadata-head {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 10px;
  flex-wrap: wrap;
}

.label {
  font-weight: 600;
  font-size: 13px;
}

.hint {
  font-size: 11px;
  color: var(--text-dim);
}

/* 对齐歌词区的虚线触发按钮（design §6.2） */
.fallback-trigger {
  margin-bottom: 10px;
  width: 100%;
  padding: 6px 0;
  background: transparent;
  border: 1px dashed var(--border);
  border-radius: 6px;
  color: var(--text-dim);
  font-size: 11.5px;
  transition: border-color 0.12s, color 0.12s, transform 0.05s;
}

.fallback-trigger:hover:not(:disabled) {
  border-color: var(--accent);
  color: var(--accent);
}

.fallback-trigger:active {
  transform: translateY(1px);
}

.fallback-trigger:disabled {
  opacity: 0.5;
  cursor: not-allowed;
  transform: none;
}

.fallback-note {
  margin: -4px 0 10px;
  font-size: 10.5px;
  color: var(--text-dim);
}

.cand-status {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 8px 0 10px;
  font-size: 11.5px;
  color: var(--text-dim);
}

.spinner {
  width: 10px;
  height: 10px;
  border: 2px solid var(--border);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

.cand-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-bottom: 10px;
}

/* design §6.5：来源标签 + 值 + 歌名—作者，hover 琥珀 */
.cand-row {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 8px;
  background: transparent;
  border: 1px solid var(--border);
  border-radius: 6px;
  color: var(--text);
  font-size: 12px;
  text-align: left;
  cursor: pointer;
  transition: border-color 0.12s, background 0.12s;
}

.cand-row:hover {
  border-color: var(--accent);
  background: var(--hover);
}

.cand-row:active {
  transform: translateY(1px);
}

.src-tag {
  flex: 0 0 auto;
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 8px;
  background: var(--panel-2);
  border: 1px solid var(--border);
  color: var(--text-dim);
}

.cand-values {
  flex: 0 0 auto;
  font-size: 11px;
  font-weight: 600;
  color: var(--accent);
}

.cand-text {
  min-width: 0;
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-dim);
}

.score {
  flex: 0 0 auto;
  font-size: 10px;
  color: var(--text-dim);
}

.cand-empty {
  padding: 8px 0 10px;
  text-align: center;
  font-size: 11px;
  color: var(--text-dim);
}
</style>
