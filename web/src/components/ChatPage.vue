<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import { ElMessage, ElMessageBox } from 'element-plus';
import { Delete, Plus, Promotion } from '@element-plus/icons-vue';
import { appStore, initApp, persist } from '../lib/appStore';
import { currentAiSettings, resolveActiveClient, workspacePathOf, type AgentClient } from '../lib/agentClient';
import { inHost } from '../lib/otools';
import { buildSession } from '../lib/store';
import { buildSystemPrompt, buildMessages, buildUserPrompt, type PromptAssets } from '../lib/prompt';
import { fetchAsset } from '../lib/assets';
import { loadSkillBody } from '../lib/skills';
import { renderMarkdown } from '../lib/markdown';
import type { ChatSession, Profile } from '../types';

const sessions = computed(() =>
  [...appStore.state.sessions].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)),
);
const activeSession = computed(
  () => appStore.state.sessions.find((session) => session.id === appStore.state.activeSessionId) ?? null,
);
const activeProfile = computed(
  (): Profile | null => appStore.state.profiles.find((profile) => profile.id === currentProfileId.value) ?? null,
);

// 会话级画像选择:默认跟全局激活画像,会话内可独立改。
const currentProfileId = ref<string | null>(appStore.state.activeProfileId);
watch(
  () => appStore.state.activeProfileId,
  (value) => {
    if (activeSession.value && !activeSession.value.messages.length) {
      currentProfileId.value = value;
    }
  },
);

const input = ref('');
const busy = ref(false);
const streaming = ref(false);
const activeSessionId = ref('');
const messagesEl = ref<HTMLElement | null>(null);
const engine = ref<AgentClient>({ id: 'chat-only', label: '仅聊天模式', streaming: false, run: async () => '' });

let promptAssets: PromptAssets | null = null;
async function ensurePromptAssets(): Promise<PromptAssets> {
  if (!promptAssets) {
    const [soul, agents] = await Promise.all([
      fetchAsset('agents/workspace/SOUL.md'),
      fetchAsset('agents/workspace/AGENTS.md'),
    ]);
    promptAssets = { soul, agents };
  }
  return promptAssets;
}

function scrollToBottom(): void {
  void nextTick(() => {
    const el = messagesEl.value;
    if (el) {
      el.scrollTop = el.scrollHeight;
    }
  });
}

function touch(session: ChatSession): void {
  session.updatedAt = new Date().toISOString();
  persist();
}

function createSession(): void {
  const session = buildSession(appStore.state.activeProfileId);
  currentProfileId.value = appStore.state.activeProfileId;
  appStore.state.sessions.push(session);
  appStore.state.activeSessionId = session.id;
  persist();
}

function selectSession(id: string): void {
  appStore.state.activeSessionId = id;
  const session = appStore.state.sessions.find((item) => item.id === id);
  currentProfileId.value = session?.profileId ?? appStore.state.activeProfileId;
  persist();
  scrollToBottom();
}

async function removeSession(id: string): Promise<void> {
  try {
    await ElMessageBox.confirm('删除后该对话的本地记录不可恢复,确定删除?', '删除对话', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    });
  } catch {
    return;
  }
  const index = appStore.state.sessions.findIndex((session) => session.id === id);
  if (index >= 0) {
    appStore.state.sessions.splice(index, 1);
  }
  if (appStore.state.activeSessionId === id) {
    appStore.state.activeSessionId = appStore.state.sessions[0]?.id ?? null;
  }
  persist();
}

function ensureSession(): ChatSession {
  let session = activeSession.value;
  if (!session) {
    createSession();
    session = activeSession.value;
  }
  return session as ChatSession;
}

async function send(): Promise<void> {
  const text = input.value.trim();
  if (!text || busy.value) {
    return;
  }
  if (!inHost()) {
    ElMessage.warning('当前是浏览器预览,宿主 AI 命令不可用;请在 OTools 内打开本插件');
    return;
  }
  const client = engine.value;
  const session = ensureSession();
  const profile = activeProfile.value;
  const skillName = appStore.pendingSkill?.name ?? null;
  const skillPath = appStore.pendingSkill?.path ?? null;

  session.profileId = currentProfileId.value;
  session.messages.push({
    id: `msg_${Date.now()}_${Math.random().toString(16).slice(2, 8)}`,
    role: 'user',
    content: text,
    skillName: skillName ?? undefined,
    createdAt: new Date().toISOString(),
  });
  if (session.title === '新对话') {
    session.title = text.slice(0, 24);
  }
  input.value = '';
  appStore.pendingSkill = null;
  busy.value = true;
  streaming.value = false;
  activeSessionId.value = session.id;
  touch(session);
  scrollToBottom();

  const assistantMessage = {
    id: `msg_${Date.now()}_${Math.random().toString(16).slice(2, 8)}`,
    role: 'assistant' as const,
    content: '',
    reasoning: '',
    createdAt: new Date().toISOString(),
  };

  try {
    const [assets, skillContext, settings] = await Promise.all([
      ensurePromptAssets(),
      skillPath ? loadSkillBody(skillPath) : Promise.resolve(null),
      client.id === 'native-stream' ? currentAiSettings() : Promise.resolve(null),
    ]);
    if (client.id === 'native-stream' && settings && !settings.configured) {
      throw new Error('宿主 AI 配置不完整:请到 OTools 设置 → AI 填写 API 地址与模型');
    }
    // hostCodex:先做会话准备(同步技能资产到工作区),工作区路径进运行时说明。
    if (client.id === 'host-codex' && client.prepareSession) {
      try {
        await client.prepareSession(session.id);
      } catch {
        /* 工作区未就绪时按降级说明运行 */
      }
    }
    const system = buildSystemPrompt(assets, profile, skillContext, {
      engine: client.id,
      workspacePath: workspacePathOf(session.id),
    });

    if (client.streaming && client.runStream) {
      // 流式真多轮:占位消息先入列,增量直接填充。
      // sessionId 用对话 ID:hostCodex 以此复用原生 thread,nativeStream 以此关联事件。
      session.messages.push({ ...assistantMessage });
      streaming.value = true;
      scrollToBottom();
      await client.runStream(
        {
          sessionId: session.id,
          system,
          user: text,
          messages: buildMessages(session.messages.slice(0, -2), text, profile),
          temperature: 0.7,
          settings: settings ?? undefined,
        },
        {
          onDelta: (delta) => {
            const target = session.messages[session.messages.length - 1];
            target.content += delta;
            scrollToBottom();
          },
          onReasoning: (delta) => {
            const target = session.messages[session.messages.length - 1];
            target.reasoning = (target.reasoning ?? '') + delta;
          },
        },
      );
      const target = session.messages[session.messages.length - 1];
      if (!target.content.trim()) {
        target.content = '(空回复)';
      }
      // hostCodex:列出一轮产物(outputs/)。
      if (client.listOutputs && client.id === 'host-codex') {
        try {
          const outputs = await client.listOutputs(session.id);
          if (outputs.length) {
            target.outputs = outputs;
          }
        } catch {
          /* 产物列举失败不影响本轮结果 */
        }
      }
    } else {
      // 仅聊天回退:单轮接口 + 历史扁平化。
      const user = buildUserPrompt(session.messages.slice(0, -1), text, profile);
      const reply = await client.run({ system, user });
      session.messages.push({
        ...assistantMessage,
        content: reply.trim() || '(空回复)',
      });
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const last = session.messages[session.messages.length - 1];
    if (last && last.role === 'assistant' && last.content === '' && !last.error) {
      // 流式占位消息仍为空:标记为错误,避免留下一颗空气泡。
      last.content = message;
      last.error = true;
    } else {
      session.messages.push({
        id: `err_${Date.now()}`,
        role: 'assistant',
        content: message,
        error: true,
        createdAt: new Date().toISOString(),
      });
    }
  } finally {
    busy.value = false;
    streaming.value = false;
    touch(session);
    scrollToBottom();
  }
}

async function stopStreaming(): Promise<void> {
  const client = engine.value;
  if (client.abort && activeSessionId.value) {
    await client.abort(activeSessionId.value);
  }
}

function onInputKeydown(event: KeyboardEvent): void {
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    void send();
  }
}

function formatSize(size: number): string {
  if (size >= 1048576) {
    return `${(size / 1048576).toFixed(1)} MB`;
  }
  if (size >= 1024) {
    return `${(size / 1024).toFixed(1)} KB`;
  }
  return `${size} B`;
}

onMounted(async () => {
  // 等初始化完成再建会话,避免空状态会话被异步加载的持久化状态覆盖。
  await initApp();
  void resolveActiveClient().then((client) => {
    engine.value = client;
  });
  if (!appStore.state.sessions.length) {
    createSession();
  } else if (!appStore.state.activeSessionId) {
    appStore.state.activeSessionId = sessions.value[0]?.id ?? null;
  }
  scrollToBottom();
});
</script>

<template>
  <div class="chat-page">
    <aside class="session-side">
      <el-button type="primary" :icon="Plus" size="small" class="new-session" @click="createSession">
        新对话
      </el-button>
      <div class="session-list">
        <div
          v-for="session in sessions"
          :key="session.id"
          class="session-item"
          :class="{ active: session.id === appStore.state.activeSessionId }"
          @click="selectSession(session.id)"
        >
          <div class="session-title">{{ session.title }}</div>
          <el-icon class="session-delete" @click.stop="removeSession(session.id)"><Delete /></el-icon>
        </div>
      </div>
    </aside>

    <section class="chat-main">
      <div class="chat-toolbar">
        <el-select
          v-model="currentProfileId"
          placeholder="对话画像(可选)"
          size="small"
          clearable
          class="profile-select"
        >
          <el-option
            v-for="profile in appStore.state.profiles"
            :key="profile.id"
            :label="profile.name"
            :value="profile.id"
          />
        </el-select>
        <el-tag v-if="activeProfile" size="small" type="primary" effect="plain">
          画像:{{ activeProfile.name }}
        </el-tag>
        <el-tag v-if="appStore.pendingSkill" size="small" type="warning" effect="plain" closable @close="appStore.pendingSkill = null">
          技能:{{ appStore.pendingSkill.name }}
        </el-tag>
        <span class="toolbar-spacer" />
        <el-tag v-if="engine.streaming" size="small" type="success" effect="plain">流式</el-tag>
        <span class="engine-tag">{{ engine.label }}</span>
      </div>

      <div ref="messagesEl" class="messages">
        <template v-if="activeSession">
          <div v-if="!activeSession.messages.length" class="empty-hint">
            <p>从一个想法开始:告诉我账号想发什么,或先到「画像」建立账号设定、到「技能库」挑一个技能带入。</p>
          </div>
          <div
            v-for="message in activeSession.messages"
            :key="message.id"
            class="message-row"
            :class="message.role"
          >
            <div class="bubble" :class="[{ error: message.error }, message.role]">
              <div v-if="message.skillName" class="skill-chip">技能 · {{ message.skillName }}</div>
              <details v-if="message.reasoning" class="reasoning">
                <summary>思考过程</summary>
                <div class="md reasoning-body" v-html="renderMarkdown(message.reasoning)" />
              </details>
              <div class="md" v-html="renderMarkdown(message.content)" />
              <div v-if="message.outputs?.length" class="outputs">
                <div class="outputs-title">工作区产物(outputs/)</div>
                <div v-for="file in message.outputs" :key="file.path" class="output-item">
                  <span class="output-path">{{ file.path }}</span>
                  <span class="output-size">{{ formatSize(file.size) }}</span>
                </div>
              </div>
            </div>
          </div>
          <div v-if="busy && !streaming" class="message-row assistant">
            <div class="bubble assistant pending">Easel 正在构思…</div>
          </div>
        </template>
      </div>

      <div class="composer">
        <el-input
          v-model="input"
          type="textarea"
          :rows="3"
          resize="none"
          :disabled="busy"
          placeholder="描述你想做的内容;Enter 发送,Shift+Enter 换行"
          @keydown="onInputKeydown"
        />
        <el-button
          v-if="streaming"
          type="danger"
          plain
          :disabled="!engine.abort"
          class="send-btn"
          @click="stopStreaming"
        >
          停止
        </el-button>
        <el-button
          v-else
          type="primary"
          :icon="Promotion"
          :loading="busy"
          :disabled="!input.trim()"
          class="send-btn"
          @click="send"
        >
          发送
        </el-button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.chat-page {
  display: flex;
  height: 100%;
}
.session-side {
  width: 200px;
  flex: none;
  border-right: 1px solid var(--el-border-color-lighter);
  display: flex;
  flex-direction: column;
  padding: 10px;
  gap: 10px;
}
.new-session {
  width: 100%;
}
.session-list {
  flex: 1;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.session-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 7px 10px;
  border-radius: 6px;
  cursor: pointer;
  color: var(--el-text-color-regular);
}
.session-item:hover {
  background: var(--el-fill-color-light);
}
.session-item.active {
  background: var(--el-color-primary-light-9);
  color: var(--el-color-primary);
}
.session-title {
  flex: 1;
  font-size: 13px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.session-delete {
  flex: none;
  color: var(--el-text-color-placeholder);
  visibility: hidden;
}
.session-item:hover .session-delete {
  visibility: visible;
}
.session-delete:hover {
  color: var(--el-color-danger);
}
.chat-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
.chat-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 14px;
  border-bottom: 1px solid var(--el-border-color-lighter);
  flex: none;
}
.profile-select {
  width: 180px;
}
.toolbar-spacer {
  flex: 1;
}
.engine-tag {
  font-size: 12px;
  color: var(--el-text-color-secondary);
}
.messages {
  flex: 1;
  overflow: auto;
  padding: 18px 20px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.empty-hint {
  margin: auto;
  color: var(--el-text-color-secondary);
  text-align: center;
  max-width: 420px;
}
.message-row {
  display: flex;
}
.message-row.user {
  justify-content: flex-end;
}
.message-row.assistant {
  justify-content: flex-start;
}
.bubble {
  max-width: 76%;
  border-radius: 10px;
  padding: 9px 12px;
  background: var(--el-fill-color-light);
}
.bubble.user {
  background: var(--el-color-primary);
  color: #fff;
}
.bubble.error {
  background: var(--el-color-danger-light-9);
  color: var(--el-color-danger);
  border: 1px solid var(--el-color-danger-light-5);
}
.bubble.pending {
  color: var(--el-text-color-secondary);
}
.skill-chip {
  display: inline-block;
  font-size: 12px;
  padding: 0 6px;
  border-radius: 4px;
  margin-bottom: 6px;
  background: rgb(255 255 255 / 18%);
}
.composer {
  display: flex;
  align-items: flex-end;
  gap: 10px;
  padding: 12px 14px;
  border-top: 1px solid var(--el-border-color-lighter);
  flex: none;
}
.composer :deep(.el-textarea__inner) {
  --el-input-bg-color: var(--el-fill-color-blank);
}
.send-btn {
  height: 54px;
}
.reasoning {
  margin: 0 0 6px;
  font-size: 12px;
  color: var(--el-text-color-secondary);
}
.reasoning summary {
  cursor: pointer;
  user-select: none;
}
.reasoning-body {
  margin-top: 4px;
  padding-left: 8px;
  border-left: 2px solid var(--el-border-color);
  opacity: 0.85;
}
.outputs {
  margin-top: 8px;
  padding: 8px 10px;
  border: 1px dashed var(--el-border-color);
  border-radius: 8px;
  background: var(--el-fill-color-lighter);
}
.outputs-title {
  font-size: 12px;
  color: var(--el-text-color-secondary);
  margin-bottom: 4px;
}
.output-item {
  display: flex;
  justify-content: space-between;
  gap: 10px;
  font-size: 12px;
  padding: 1px 0;
}
.output-path {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  word-break: break-all;
}
.output-size {
  flex: none;
  color: var(--el-text-color-secondary);
}
</style>
