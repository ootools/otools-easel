import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const scripts = fileURLToPath(new URL('../public/easel/skills/shared/scripts/', import.meta.url));
function python(code: string) {
  const result = spawnSync('python3', ['-I', '-B', '-c', code, scripts], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr || String(result.error));
}
const bootstrap = String.raw`
import argparse, importlib.util, json, os, sys, tempfile
from pathlib import Path
sys.path.insert(0, sys.argv[1])
def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(sys.argv[1]) / f'{name}.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module
os.environ['OTOOLS_AI_MEDIA_MANAGED'] = '1'
workspace = tempfile.TemporaryDirectory()
os.environ['EASEL_ROOT'] = workspace.name
`;

test('managed media does not resurrect stored model or credential aliases', () => {
  python(bootstrap + String.raw`
with tempfile.TemporaryDirectory() as directory:
    os.environ['EASEL_ROOT'] = directory
    env_file = Path(directory) / '.env'
    env_file.write_text('IMG_MODEL=old-image\nIMG_API_KEY=old-secret\nOPENAI_MODEL=old-alias\nOPENAI_API_KEY=old-alias-key\nVIDEO_MODEL=old-video\nVOICE_MODEL=old-voice\nEASEL_PROXY=http://localhost:7890\n')
    for key in ['IMG_MODEL','IMG_API_KEY','VIDEO_MODEL','VIDEO_API_KEY','VOICE_MODEL','VOICE_API_KEY']:
        os.environ[key] = ''
    os.environ['OPENAI_MODEL'] = 'old-alias'
    os.environ['OPENAI_API_KEY'] = 'old-alias-key'
    image = load('ai_image')
    image.load_env_file(env_file)
    assert image.resolve_config('IMG_MODEL')[0] == ''
    assert image.resolve_config('IMG_API_KEY')[0] == ''
    video = load('ai_video')
    video.load_env_file(env_file)
    assert video.env_lookup('VIDEO_MODEL') == ''
    voice = load('voice_clone')
    voice.load_env_file(env_file)
    assert voice.env_lookup('VOICE_MODEL') == ''
    registry = load('model_registry')
    values = registry.read_env_file(env_file)
    assert values.get('IMG_MODEL','') == ''
    assert values['EASEL_PROXY'] == 'http://localhost:7890'
    os.environ['IMG_MODEL'] = 'chosen-image'
    os.environ['IMG_API_KEY'] = 'fixture-key'
    assert image.resolve_config('IMG_MODEL')[0] == 'chosen-image'
`);
});

test('managed scripts reject alternate provider credentials and cannot disable the managed guard from .env', () => {
  python(bootstrap + String.raw`
with tempfile.TemporaryDirectory() as directory:
    env_file = Path(directory) / '.env'
    env_file.write_text('GEMINI_API_KEY=old-key\nGEMINI_TTS_MODEL=old-model\nVOICE_PROVIDER=gemini\nVIDEO_PROVIDER=dashscope\nDASHSCOPE_API_KEY=old-key\nOTOOLS_AI_MEDIA_MANAGED=0\nEASEL_PROXY=http://localhost:7890\n')
    for key in ['GEMINI_API_KEY','GEMINI_TTS_MODEL','DASHSCOPE_API_KEY','VOICE_PROVIDER','VIDEO_PROVIDER']:
        os.environ.pop(key, None)
    os.environ['VOICE_MODEL'] = ''
    voice = load('voice_clone')
    voice.load_env_file(env_file)
    assert os.environ.get('GEMINI_API_KEY', '') == '', 'managed loaders must not import alternate provider credentials'
    assert os.environ['OTOOLS_AI_MEDIA_MANAGED'] == '1'
    try:
        voice.resolve_provider('gemini')
    except SystemExit:
        pass
    else:
        raise AssertionError('managed speech must not execute an alternate provider')
    video = load('ai_video')
    video.load_env_file(env_file)
    assert video.env_lookup('DASHSCOPE_API_KEY') == ''
    assert os.environ.get('EASEL_PROXY') == 'http://localhost:7890'
`);
});

test('managed video and speech require a selected model rather than built-in defaults', () => {
  python(bootstrap + String.raw`
os.environ['VIDEO_MODEL'] = ''
video = load('ai_video')
assert video.resolve_model('openai-compatible') == '', 'unselected video must not use sora-1'
assert video.resolve_model('openai-compatible', 'chosen-video') == 'chosen-video'
voice = load('voice_clone')
os.environ['VOICE_MODEL'] = ''
os.environ['VOICE_API_KEY'] = 'fixture-key'
os.environ['VOICE_BASE_URL'] = 'http://127.0.0.1:9/v1'
sent = []
class Response:
    def __enter__(self): return self
    def __exit__(self, *args): pass
    def read(self): return b'fixture-audio'
def open_request(request, **kwargs):
    sent.append(json.loads(request.data))
    return Response()
voice.urllib.request.urlopen = open_request
with tempfile.TemporaryDirectory() as directory:
    args = argparse.Namespace(model=None,voice_id=None,text='fixture',speed=1,emotion=None)
    try:
        voice.clone_openai(args, Path(directory) / 'fixture.mp3')
    except SystemExit:
        pass
    else:
        raise AssertionError('unselected speech must fail before sending a request')
    assert sent == []
    args.model = 'chosen-voice'
    voice.clone_openai(args, Path(directory) / 'fixture.mp3')
    assert sent[0]['model'] == 'chosen-voice'
`);
});
