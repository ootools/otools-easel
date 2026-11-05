import assert from 'node:assert/strict';
import { test } from 'node:test';
import { resolveEmbeddedPath } from '../src/lib/media.ts';

test('HTML previews resolve local assets relative to the original output file', () => {
  assert.equal(resolveEmbeddedPath('项目/卡片/index.html', '../素材/封面.png?x=1'), '项目/素材/封面.png');
  assert.equal(resolveEmbeddedPath('项目/卡片/index.html', './styles.css'), '项目/卡片/styles.css');
  assert.equal(resolveEmbeddedPath('项目/卡片/index.html', '/api/media/%E9%A1%B9%E7%9B%AE/cover.png'), '项目/cover.png');
  assert.equal(resolveEmbeddedPath('项目/卡片/index.html', '#section'), null);
  assert.equal(resolveEmbeddedPath('项目/卡片/index.html', 'https://example.org/image.png'), null);
  assert.equal(resolveEmbeddedPath('项目/卡片/index.html', 'data:image/png;base64,AA=='), null);
  assert.throws(() => resolveEmbeddedPath('项目/index.html', '../../secret'));
  assert.throws(() => resolveEmbeddedPath('项目/index.html', 'javascript:alert(1)'));
});
