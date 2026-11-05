<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { appStore } from '../lib/appStore';
import { layerLabel, loadSkillBody, loadSkillIndex } from '../lib/skills';
import { renderMarkdown } from '../lib/markdown';
import type { SkillMeta } from '../types';

const skills = ref<SkillMeta[]>([]);
const loading = ref(true);
const loadError = ref('');
const keyword = ref('');
const layerFilter = ref('');

const layers = computed(() => {
  const set = new Set(skills.value.map((skill) => skill.layer).filter(Boolean));
  return [...set];
});

const filtered = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  return skills.value.filter((skill) => {
    if (layerFilter.value && skill.layer !== layerFilter.value) {
      return false;
    }
    if (!kw) {
      return true;
    }
    return (
      skill.name.toLowerCase().includes(kw) ||
      skill.description.toLowerCase().includes(kw)
    );
  });
});

const drawerVisible = ref(false);
const detailSkill = ref<SkillMeta | null>(null);
const detailHtml = ref('');
const detailLoading = ref(false);

async function openDetail(skill: SkillMeta): Promise<void> {
  detailSkill.value = skill;
  detailHtml.value = '';
  drawerVisible.value = true;
  detailLoading.value = true;
  try {
    detailHtml.value = renderMarkdown(await loadSkillBody(skill.path));
  } catch (error) {
    detailHtml.value = `<p>加载失败:${error instanceof Error ? error.message : String(error)}</p>`;
  } finally {
    detailLoading.value = false;
  }
}

async function useInChat(): Promise<void> {
  if (!detailSkill.value) {
    return;
  }
  appStore.pendingSkill = { name: detailSkill.value.name, path: detailSkill.value.path };
  appStore.activePage = 'chat';
  ElMessage.success(`技能「${detailSkill.value.name}」已带入对话,发送消息后生效`);
}

onMounted(async () => {
  try {
    skills.value = await loadSkillIndex();
  } catch (error) {
    loadError.value = error instanceof Error ? error.message : String(error);
  } finally {
    loading.value = false;
  }
});
</script>

<template>
  <div class="skills-page">
    <div class="page-toolbar">
      <el-input
        v-model="keyword"
        placeholder="搜索技能名称或描述"
        size="small"
        clearable
        class="search-input"
      />
      <el-select v-model="layerFilter" placeholder="全部层级" size="small" clearable class="layer-select">
        <el-option v-for="layer in layers" :key="layer" :label="layerLabel(layer)" :value="layer" />
      </el-select>
      <span class="count-hint">{{ loading ? '加载中…' : `共 ${filtered.length} 个技能` }}</span>
    </div>

    <el-alert v-if="loadError" type="error" :title="`技能库加载失败:${loadError}`" :closable="false" />

    <div v-else class="skill-grid">
      <div v-for="skill in filtered" :key="skill.path" class="skill-card" @click="openDetail(skill)">
        <div class="skill-head">
          <span class="skill-name">{{ skill.name }}</span>
          <el-tag v-if="skill.layer" size="small" effect="plain">{{ layerLabel(skill.layer) }}</el-tag>
        </div>
        <div class="skill-desc">{{ skill.description || '(无描述)' }}</div>
      </div>
    </div>

    <el-drawer v-model="drawerVisible" :title="detailSkill?.name" size="56%">
      <div v-if="detailLoading" class="detail-loading">加载中…</div>
      <div v-else class="md detail-body" v-html="detailHtml" />
      <template #footer>
        <el-button type="primary" :disabled="detailLoading" @click="useInChat">带入对话</el-button>
      </template>
    </el-drawer>
  </div>
</template>

<style scoped>
.skills-page {
  height: 100%;
  overflow: auto;
  padding: 16px 20px;
}
.page-toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 14px;
}
.search-input {
  width: 260px;
}
.layer-select {
  width: 130px;
}
.count-hint {
  font-size: 12px;
  color: var(--el-text-color-secondary);
}
.skill-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: 10px;
}
.skill-card {
  border: 1px solid var(--el-border-color-lighter);
  border-radius: 10px;
  padding: 12px 14px;
  cursor: pointer;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.skill-card:hover {
  border-color: var(--el-color-primary-light-5);
  background: var(--el-fill-color-light);
}
.skill-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}
.skill-name {
  font-weight: 600;
  font-size: 14px;
}
.skill-desc {
  font-size: 12px;
  color: var(--el-text-color-secondary);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.detail-loading {
  color: var(--el-text-color-secondary);
  padding: 20px;
}
.detail-body {
  max-width: 860px;
}
</style>
