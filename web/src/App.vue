<script setup lang="ts">
import { computed, onMounted } from 'vue';
import { appStore, initApp } from './lib/appStore';
import { inHost } from './lib/otools';
import ChatPage from './components/ChatPage.vue';
import ProfilesPage from './components/ProfilesPage.vue';
import SkillsPage from './components/SkillsPage.vue';

const pages = [
  { key: 'chat', label: '对话' },
  { key: 'profiles', label: '画像' },
  { key: 'skills', label: '技能库' },
] as const;

const aiStatus = computed(() => {
  if (!appStore.loaded) {
    return { text: '检测宿主配置中…', tone: 'info' as const };
  }
  if (!inHost()) {
    return { text: '浏览器预览:宿主功能不可用', tone: 'warning' as const };
  }
  if (appStore.aiInfo?.configured) {
    return { text: `AI:${appStore.aiInfo.model || appStore.aiInfo.provider}`, tone: 'success' as const };
  }
  return { text: 'AI 未配置:请到 OTools 设置 → AI 完成配置', tone: 'warning' as const };
});

onMounted(() => {
  void initApp();
});
</script>

<template>
  <div class="app-shell">
    <header class="app-header">
      <div class="brand">
        <span class="brand-logo">🎨</span>
        <span class="brand-name">Easel 内容工坊</span>
      </div>
      <nav class="app-nav">
        <button
          v-for="page in pages"
          :key="page.key"
          class="nav-item"
          :class="{ active: appStore.activePage === page.key }"
          type="button"
          @click="appStore.activePage = page.key"
        >
          {{ page.label }}
        </button>
      </nav>
      <div class="header-status">
        <span class="status-dot" :class="aiStatus.tone" />
        <span class="status-text">{{ aiStatus.text }}</span>
      </div>
    </header>
    <main class="app-main">
      <div v-if="!appStore.loaded" class="boot-loading">
        <span>正在加载 Easel…</span>
      </div>
      <KeepAlive v-else>
        <ChatPage v-if="appStore.activePage === 'chat'" />
        <ProfilesPage v-else-if="appStore.activePage === 'profiles'" />
        <SkillsPage v-else />
      </KeepAlive>
    </main>
  </div>
</template>

<style scoped>
.app-shell {
  display: flex;
  flex-direction: column;
  height: 100%;
}
.app-header {
  display: flex;
  align-items: center;
  gap: 20px;
  padding: 0 16px;
  height: 48px;
  border-bottom: 1px solid var(--el-border-color-lighter);
  flex: none;
}
.brand {
  display: flex;
  align-items: center;
  gap: 8px;
  font-weight: 600;
  min-width: 180px;
}
.brand-logo {
  font-size: 18px;
}
.app-nav {
  display: flex;
  gap: 4px;
  flex: 1;
}
.nav-item {
  border: none;
  background: transparent;
  color: var(--el-text-color-secondary);
  font-size: 14px;
  padding: 6px 14px;
  border-radius: 6px;
  cursor: pointer;
}
.nav-item:hover {
  background: var(--el-fill-color-light);
  color: var(--el-text-color-primary);
}
.nav-item.active {
  background: var(--el-color-primary-light-9);
  color: var(--el-color-primary);
  font-weight: 600;
}
.header-status {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 180px;
  justify-content: flex-end;
}
.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex: none;
}
.status-dot.success {
  background: var(--el-color-success);
}
.status-dot.warning {
  background: var(--el-color-warning);
}
.status-dot.info {
  background: var(--el-color-info);
}
.status-text {
  font-size: 12px;
  color: var(--el-text-color-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.app-main {
  flex: 1;
  min-height: 0;
}
.boot-loading {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--el-text-color-secondary);
}
</style>
