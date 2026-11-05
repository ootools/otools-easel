export type ProfileFileKey = 'identity' | 'style' | 'audience' | 'platforms' | 'preferences' | 'memory';

export interface Profile {
  id: string;
  name: string;
  files: Record<ProfileFileKey, string>;
  createdAt: string;
  updatedAt: string;
}

export const PROFILE_FILE_KEYS: ProfileFileKey[] = ['identity', 'style', 'audience', 'platforms', 'preferences', 'memory'];

export const PROFILE_FILE_LABELS: Record<ProfileFileKey, string> = {
  identity: '定位',
  style: '风格',
  audience: '受众',
  platforms: '平台',
  preferences: '偏好与红线',
  memory: '长期记忆',
};

export interface ChatMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  /** 思考模型(reasoning)增量,native 流式时可能存在 */
  reasoning?: string;
  /** 本条消息创作时带入的技能名(仅 user 消息) */
  skillName?: string;
  /** Agent 工作区产物(hostCodex 完成本轮后列出) */
  outputs?: { path: string; size: number }[];
  /** 生成失败的占位消息,不参与后续上下文拼接 */
  error?: boolean;
  createdAt: string;
}

export interface ChatSession {
  id: string;
  title: string;
  profileId: string | null;
  messages: ChatMessage[];
  createdAt: string;
  updatedAt: string;
}

export interface SkillMeta {
  /** 相对 easel/ 资产根的路径,如 skills/builtin/social-content/SKILL.md */
  path: string;
  name: string;
  description: string;
  layer: string;
}

export interface PluginState {
  profiles: Profile[];
  sessions: ChatSession[];
  activeProfileId: string | null;
  activeSessionId: string | null;
}
