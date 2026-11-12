<script setup lang="ts">
// 在线歌词候选条（online-lyrics-sources）：来源标签（千千 Plus / Rangotec）+ 歌名 — 作者 + 专辑。
// 与主搜索的 LyricCandidate.vue 分开：在线源不属于 MusicSourceId，不参与 C2 换源，
// 点击 → store.pickOnlineLyric（点选才 fetch_online_lyric 拉正文，惰性拉取）。
import type { LyricCandidate } from '../api/types'
import { lyricsProviderLabel } from '../store/selectors'
import { pickOnlineLyric } from '../store/song'

defineProps<{
  cand: LyricCandidate
  disabled: boolean
}>()
</script>

<template>
  <button
    type="button"
    class="online-cand-row"
    :title="`${cand.title} — ${cand.artist}${cand.album ? ` · ${cand.album}` : ''}`"
    :disabled="disabled"
    @click="pickOnlineLyric(cand)"
  >
    <span class="src-tag">{{ lyricsProviderLabel(cand.provider) }}</span>
    <span class="cand-text">{{ cand.title }} — {{ cand.artist }}</span>
    <span v-if="cand.album" class="cand-album">{{ cand.album }}</span>
  </button>
</template>

<style scoped>
.online-cand-row {
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

.online-cand-row:hover:not(:disabled) {
  border-color: var(--accent);
  background: var(--hover);
}

.online-cand-row:active:not(:disabled) {
  transform: translateY(1px);
}

.online-cand-row:disabled {
  opacity: 0.55;
  cursor: progress;
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

.cand-text {
  min-width: 0;
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cand-album {
  flex: 0 1 30%;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 10.5px;
  color: var(--text-dim);
}
</style>
