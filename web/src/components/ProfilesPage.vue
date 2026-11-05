<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { ElMessage, ElMessageBox } from 'element-plus';
import { Delete, Edit, Plus } from '@element-plus/icons-vue';
import { appStore, persist } from '../lib/appStore';
import { buildProfile } from '../lib/store';
import { fetchAsset, loadManifest } from '../lib/assets';
import { PROFILE_FILE_KEYS, PROFILE_FILE_LABELS, type Profile, type ProfileFileKey } from '../types';

const createDialogVisible = ref(false);
const newName = ref('');

const editing = ref<Profile | null>(null);
const editVisible = computed({
  get: () => !!editing.value,
  set: (value: boolean) => {
    if (!value) {
      editing.value = null;
    }
  },
});
const editDraft = ref<Record<ProfileFileKey, string>>({ identity: '', style: '', audience: '', platforms: '', preferences: '', memory: '' });

let templateFiles: Record<ProfileFileKey, string> | null = null;
async function ensureTemplateFiles(): Promise<Record<ProfileFileKey, string>> {
  if (!templateFiles) {
    const manifest = await loadManifest();
    const entries = await Promise.all(
      PROFILE_FILE_KEYS.map(async (key) => {
        const path = manifest.profiles.find((item) => item.endsWith(`/${key}.md`));
        return [key, path ? await fetchAsset(path) : ''] as const;
      }),
    );
    templateFiles = Object.fromEntries(entries) as Record<ProfileFileKey, string>;
  }
  return templateFiles;
}

function openCreate(): void {
  newName.value = '';
  createDialogVisible.value = true;
}

async function confirmCreate(): Promise<void> {
  const name = newName.value.trim();
  if (!name) {
    ElMessage.warning('请先填写画像名称');
    return;
  }
  if (appStore.state.profiles.some((profile) => profile.name === name)) {
    ElMessage.warning('已存在同名画像');
    return;
  }
  const files = await ensureTemplateFiles();
  const profile = buildProfile(name, { ...files });
  appStore.state.profiles.push(profile);
  appStore.state.activeProfileId = profile.id;
  persist();
  createDialogVisible.value = false;
  ElMessage.success(`画像「${name}」已创建,默认用于新对话`);
}

function openEdit(profile: Profile): void {
  editing.value = profile;
  editDraft.value = { ...profile.files };
}

async function saveEdit(): Promise<void> {
  const profile = editing.value;
  if (!profile) {
    return;
  }
  profile.files = { ...editDraft.value };
  profile.updatedAt = new Date().toISOString();
  persist();
  editing.value = null;
  ElMessage.success('画像已保存');
}

async function removeProfile(profile: Profile): Promise<void> {
  try {
    await ElMessageBox.confirm(`删除画像「${profile.name}」?已有对话不会受影响,但将失去其画像上下文。`, '删除画像', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    });
  } catch {
    return;
  }
  const index = appStore.state.profiles.findIndex((item) => item.id === profile.id);
  if (index >= 0) {
    appStore.state.profiles.splice(index, 1);
  }
  if (appStore.state.activeProfileId === profile.id) {
    appStore.state.activeProfileId = appStore.state.profiles[0]?.id ?? null;
  }
  persist();
}

function setActive(profile: Profile): void {
  appStore.state.activeProfileId = profile.id;
  persist();
}

function formatDate(iso: string): string {
  return new Date(iso).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}

onMounted(() => {
  void ensureTemplateFiles();
});
</script>

<template>
  <div class="profiles-page">
    <div class="page-toolbar">
      <span class="page-hint">画像在对话时作为账号上下文注入;全局激活画像会作为新对话的默认选择。</span>
      <el-button type="primary" :icon="Plus" size="small" @click="openCreate">新建画像</el-button>
    </div>

    <div v-if="!appStore.state.profiles.length" class="empty-hint">
      <p>还没有画像。新建一个,用上游模板的六维结构开始:定位 / 风格 / 受众 / 平台 / 偏好红线 / 长期记忆。</p>
    </div>

    <div class="profile-grid">
      <div v-for="profile in appStore.state.profiles" :key="profile.id" class="profile-card">
        <div class="card-head">
          <span class="card-name">{{ profile.name }}</span>
          <el-tag v-if="profile.id === appStore.state.activeProfileId" size="small" type="success" effect="plain">
            对话默认
          </el-tag>
        </div>
        <div class="card-meta">更新于 {{ formatDate(profile.updatedAt) }}</div>
        <div class="card-actions">
          <el-button size="small" :icon="Edit" @click="openEdit(profile)">编辑</el-button>
          <el-button
            v-if="profile.id !== appStore.state.activeProfileId"
            size="small"
            @click="setActive(profile)"
          >
            设为对话默认
          </el-button>
          <el-button size="small" type="danger" plain :icon="Delete" @click="removeProfile(profile)">删除</el-button>
        </div>
      </div>
    </div>

    <el-dialog v-model="createDialogVisible" title="新建画像" width="420px">
      <el-input v-model="newName" placeholder="画像名称,如「科技数码达人」" maxlength="40" show-word-limit />
      <template #footer>
        <el-button @click="createDialogVisible = false">取消</el-button>
        <el-button type="primary" @click="confirmCreate">创建</el-button>
      </template>
    </el-dialog>

    <el-dialog v-model="editVisible" :title="`编辑画像:${editing?.name ?? ''}`" fullscreen class="profile-editor">
      <el-tabs>
        <el-tab-pane
          v-for="key in PROFILE_FILE_KEYS"
          :key="key"
          :label="PROFILE_FILE_LABELS[key]"
        >
          <el-input
            v-model="editDraft[key]"
            type="textarea"
            :rows="18"
            class="mono-textarea"
            :placeholder="`Markdown 内容 · ${PROFILE_FILE_LABELS[key]}`"
          />
        </el-tab-pane>
      </el-tabs>
      <template #footer>
        <el-button @click="editing = null">取消</el-button>
        <el-button type="primary" @click="saveEdit">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<style scoped>
.profiles-page {
  height: 100%;
  overflow: auto;
  padding: 16px 20px;
}
.page-toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 14px;
}
.page-hint {
  flex: 1;
  font-size: 12px;
  color: var(--el-text-color-secondary);
}
.empty-hint {
  margin: 60px auto;
  text-align: center;
  color: var(--el-text-color-secondary);
  max-width: 420px;
}
.profile-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: 12px;
}
.profile-card {
  border: 1px solid var(--el-border-color-lighter);
  border-radius: 10px;
  padding: 14px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.card-head {
  display: flex;
  align-items: center;
  gap: 8px;
}
.card-name {
  font-weight: 600;
  font-size: 15px;
}
.card-meta {
  font-size: 12px;
  color: var(--el-text-color-secondary);
}
.card-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 4px;
}
.card-actions .el-button {
  margin-left: 0;
}
</style>
