import numpy as np
from server import decode_pcm, MAX_AUDIO_BYTES

audio = decode_pcm(np.full(16000, 1000, dtype="<i2").tobytes())
assert audio.dtype == np.float32 and len(audio) == 16000
assert abs(float(audio[0]) - 1000 / 32768) < 1e-6
for bad in (b"", b"a" * 3201, b"\0" * 32000, b"a" * (MAX_AUDIO_BYTES + 2)):
    try:
        decode_pcm(bad)
    except ValueError:
        continue
    raise AssertionError("Invalid audio was accepted")
print("PCM validation passed")

# The streaming decoder must decode full UTF-8 sequences and stop cancelled jobs.
import threading
from server import transcribe_audio

class FakeModel:
    _tokenizer = None
    def stream_generate(self, *args, **kwargs):
        yield 1, None
        yield 2, None
    def decode(self, tokens):
        assert tokens == [1, 2], "Chinese token bytes were decoded separately"
        return "language Chinese<asr_text>你好"
    def extract_language(self, text):
        return "Chinese", text.split("<asr_text>")[1]

model = FakeModel()
model._tokenizer = model
assert transcribe_audio(model, audio, None, threading.Event()) == "你好"
cancelled = threading.Event()
cancelled.set()
try:
    transcribe_audio(model, audio, None, cancelled)
    raise AssertionError("Cancelled job was transcribed")
except ValueError:
    pass
print("Unicode and cancellation passed")

# Noise has no dictation result; short/quiet speech is not rejected by loudness.
from types import SimpleNamespace
from server import speech_audio, only_hesitation
class FakeVAD:
    spans = []
    def get_speech_timestamps(self, audio, **kwargs):
        assert kwargs['threshold'] == .35 and kwargs['min_speech_duration_ms'] < 150
        return self.spans
vad = FakeVAD()
runtime = SimpleNamespace(vad_model=vad)
assert speech_audio(runtime, audio, threading.Event()) is None
vad.spans = [{'start': 10, 'end': 50}, {'start': 90, 'end': 130}]
quiet_audio = audio * .001
assert np.array_equal(speech_audio(runtime, quiet_audio, threading.Event()), quiet_audio[10:130])
try:
    speech_audio(runtime, audio, cancelled)
    raise AssertionError('Cancelled noise check returned audio')
except ValueError as error:
    assert str(error) == '已取消'
for text in ['', '嗯', '嗯，嗯。', '呃，啊', '额', 'erm', 'um, hmm']:
    assert only_hesitation(text), text
for text in ['好', '嗯，今天三点开会', '0012', 'This is a number']:
    assert not only_hesitation(text), text
print('Noise gating, quiet speech, hesitation-only output and cancellation passed')

# Prose uses commas/semicolons; structured values retain their symbols.
import json
import server
from server import allowed_punctuation, refine_text
from server import normalize_digit_sequences
for raw, expected in [
    ('电话是一三八零零一二三四五六', '电话是13800123456'),
    ('验证码零零三八二一', '验证码003821'),
    ('零零零一一一', '000111'),
    ('编号幺兩〇二三四', '编号120234'),
    ('验证码零 一', '验证码01'),
    ('一 二', '12'),
    ('验证码００３８２１', '验证码003821'),
    ('phone one three eight zero zero one two three four five six', 'phone 13800123456'),
    ('zero zero seven', '007'),
    ('电话一三八零零一二三四五六，验证码零零三八二一', '电话13800123456，验证码003821'),
    ('号码是138 0012 3456', '号码是138 0012 3456'),
    ('预算100 200 300', '预算100 200 300'),
    ('一五一十地说清楚，他们三三两两过来，大概五六点到，预算一千五百，价格三点五元', '一五一十地说清楚，他们三三两两过来，大概五六点到，预算一千五百，价格三点五元'),
    ('界面\n一，录音条小一点\n二，别挡输入框\n\n声音\n一，轻声也能识别', '界面\n一，录音条小一点\n二，别挡输入框\n\n声音\n一，轻声也能识别'),
]:
    assert normalize_digit_sequences(raw) == expected, (raw, normalize_digit_sequences(raw), expected)
print('Digit sequences preserve leading zeroes, repetitions and ordinary prose')
assert allowed_punctuation('“好的！”明天见。价格3.5元，折扣5%；不是-2。') == '好的，明天见，价格3.5元，折扣5%；不是-2'
assert allowed_punctuation('Hello! Budget is 3.5, discount 5%; minus -2.', True) == 'Hello, Budget is 3.5, discount 5%; minus -2'
for raw, expected in [
    ('嗯，呃，我想明天去公司，啊', '我想明天去公司'),
    ('我呃想把方案发给你', '我想把方案发给你'),
    ('um, I uh want to keep the details', 'I want to keep the details'),
    ('不要把杂音变成嗯，我的名额还在，你明天来吗', '不要把杂音变成嗯，我的名额还在，你明天来吗'),
    ('我先打开文件，然后复制，这个文件就是最终稿', '我先打开文件，然后复制，这个文件就是最终稿'),
    ('邮件发到 alice at QQ点com，折扣百分之二十五', '邮件发到 alice@QQ.com，折扣25%'),
    ('邮箱是零零一二艾特Q Q点com', '邮箱是0012@QQ.com'),
    ('发给 first 点 last at example dot co dot uk', '发给 first.last@example.co.uk'),
    ('邮箱 dotty+tag@gmail.com，文件 README.md', '邮箱 dotty+tag@gmail.com，文件 README.md'),
    ('邮箱 user下划线01诶特example点com', '邮箱 user_01@example.com'),
    ('邮箱 user+tag@sub.example.com', '邮箱 user+tag@sub.example.com'),
    ('百分之三点五，千分之五，百分之负十二点零五', '3.5%，5‰，-12.05%'),
    ('百分之一百二十五，百分之零点零五，折扣25％', '125%，0.05%，折扣25%'),
    ('预算100 200 300，三点开会，时间 at noon', '预算100 200 300，三点开会，时间 at noon'),
]:
    # Chinese spacing is not changed around an existing ASCII word.
    actual = allowed_punctuation(raw, raw.startswith('um,'))
    assert actual == expected, (raw, actual, expected)
assert allowed_punctuation('界面\r\n一，录音条小一点。\n二，别挡输入框！\n\n\n声音\n一，轻声也能识别。') == '界面\n1，录音条小一点\n2，别挡输入框\n\n声音\n1，轻声也能识别'
assert allowed_punctuation('界面\n1，缩小\n1，图标\n\n1，保留波形\n\n识别\n1，轻声\n1，过滤杂音') == '界面\n1，缩小\n2，图标\n\n3，保留波形\n\n识别\n1，轻声\n2，过滤杂音'
assert allowed_punctuation('Interface\n1, Smaller bar\n1, Icon buttons\n\nSpeech\n1, Quiet voice\n1, No noise', True) == 'Interface\n1, Smaller bar\n2, Icon buttons\n\nSpeech\n1, Quiet voice\n2, No noise'
assert allowed_punctuation('电话0012\n验证码003821') == '电话0012\n验证码003821'
assert allowed_punctuation('Interface\n1 Make the bar smaller.\n2 Keep the input visible!\n\nVoice\n1 Preserve quiet speech.', True) == 'Interface\n1 Make the bar smaller\n2 Keep the input visible\n\nVoice\n1 Preserve quiet speech'
original_request = server.editor_request
class FakeResponse:
    def __init__(self, result, reason='stop'):
        self.result, self.reason = result, reason
    def __enter__(self): return self
    def __exit__(self, *args): pass
    def __iter__(self):
        yield json.dumps({'message': {'content': json.dumps({'text': self.result})}, 'done': False}).encode()
        yield json.dumps({'done': True, 'done_reason': self.reason}).encode()
try:
    requests = []
    def fake_editor(path, payload):
        requests.append(payload)
        return FakeResponse('今天三点开会。' if len(requests) == 1 else '复制后图标变成勾')
    server.editor_request = fake_editor
    assert refine_text('今天五点哦不对三点开会', False, threading.Event()) == '今天三点开会'
    assert refine_text('复制后图标变成勾', False, threading.Event()) == '复制后图标变成勾'
    # Each request must contain only its own dictation, never previous turns or
    # concrete sample requirements that the small model could copy into output.
    assert len(requests[1]['messages']) == 2
    assert requests[1]['messages'][1] == {'role': 'user', 'content': '听写原文："复制后图标变成勾"'}
    assert requests[0]['messages'][1] != requests[1]['messages'][1]
    assert all(request['keep_alive'] == '5m' for request in requests)
    for sample in ['把模型和配置一起打包', '离得很近', '重新下载', 'Interface\n1, Make the bar smaller']:
        assert sample not in requests[1]['messages'][0]['content'], sample
    cancelled = threading.Event(); cancelled.set()
    try:
        refine_text('test', False, cancelled)
        raise AssertionError('Cancelled edit returned text')
    except ValueError as error:
        assert str(error) == '已取消'
    server.editor_request = lambda path, payload: FakeResponse('partial', 'length')
    try:
        refine_text('test', False, threading.Event())
        raise AssertionError('Truncated edit returned text')
    except ValueError as error:
        assert '过长' in str(error)
finally:
    server.editor_request = original_request
print('Editor formatting and cancellation passed')

# Vocabulary is per user, advisory, bounded, and needs repeated independent uses.
import tempfile
from pathlib import Path
from vocabulary import Vocabulary, MAX_WORDS, MAX_HINTS
with tempfile.TemporaryDirectory() as folder:
    path = Path(folder) / 'user-words.json'
    words = Vocabulary(path)
    for _ in range(2):
        words.record(['阿里云', '阿里云', 'MD', '不存在', '13800123456'], '阿里云的MD文件', '阿里云的MD文件')
    assert words.hints() == []
    words.record(['阿里云', 'MD'], '阿里云的MD文件', '阿里云的MD文件')
    assert words.hints() == ['阿里云', 'MD']
    assert Vocabulary(path).counts == {'阿里云': 3, 'MD': 3}
    assert Vocabulary(Path(folder) / 'another-user.json').hints() == []
    assert '音频优先' in words.prompt()
    assert words.protects('阿里云的 MD 文件', '阿里云的 MD 文件')
    assert not words.protects('阿里云的 MD 文件', '玉米的文件')
    assert words.protects('玉米长在地里', '玉米长在地里'), 'Dictionary must not guess unrelated words'
    words.record(['阿里云'], '阿里云', '玉米')
    assert words.counts['阿里云'] == 3
    words.record(['SecretWord'], '密码是SecretWord', '密码是SecretWord')
    assert 'SecretWord' not in words.counts
    words.counts = {f'Tool{i}': 3 for i in range(300)}
    words.record(['NewTool'], 'NewTool', 'NewTool')
    assert len(words.counts) <= MAX_WORDS and len(words.hints()) <= MAX_HINTS
    path.write_text('broken JSON')
    broken = Vocabulary(path)
    assert broken.hints() == []
    broken.record(['QuietType'], 'QuietType', 'QuietType')
    assert path.read_text() == 'broken JSON', 'Unreadable user data must never be replaced by an empty dictionary'
    try:
        broken.manage({'action': 'add_word', 'word': 'QuietType'})
        raise AssertionError('Unreadable user data was overwritten')
    except ValueError:
        pass
    assert path.read_text() == 'broken JSON'
assert allowed_punctuation('读取 README.md 和 .md 文件，用 Qwen3.5:4b；价格3.5元。') == '读取 README.md 和 .md 文件，用 Qwen3.5:4b；价格3.5元'
try:
    words = Vocabulary(); words.confirmed = ['阿里云']
    server.editor_request = lambda path, payload: FakeResponse('我的玉米还在河水中')
    raw = '我的阿里云还在审核中，请读取 MD 文件'
    assert refine_text(raw, False, threading.Event(), words, []) == raw
    raw = '嗯，我的阿里云还在审核中，呃，请读取 MD 文件，然后把原始截图发给同事'
    assert refine_text(raw, False, threading.Event(), words, []) == '我的阿里云还在审核中，请读取 MD 文件，然后把原始截图发给同事'
    server.editor_request = lambda path, payload: FakeResponse('发到 alice at QQ com，折扣百分之二十五')
    assert refine_text('嗯，发到 alice at QQ点com，呃，折扣百分之二十五', False, threading.Event()) == '发到 alice@QQ.com，折扣25%'
finally:
    server.editor_request = original_request
print('Weak per-user vocabulary, name preservation, file identifiers and corrupt-file recovery passed')

# User-managed words and exact whole-utterance snippets share the sidecar writer.
from vocabulary import MAX_SNIPPETS
with tempfile.TemporaryDirectory() as folder:
    path = Path(folder) / 'user-words.json'
    path.write_text(json.dumps({'version': 1, 'counts': {'阿里云': 3}, 'confirmed': [], 'futureSetting': True}))
    words = Vocabulary(path)
    result = words.manage({'action': 'add_word', 'word': '阿里云'})
    assert result['words'] == [{'word': '阿里云', 'count': 3, 'confirmed': True}]
    words.manage({'action': 'add_word', 'word': 'QuietType'})
    for word in ['quiettype', 'Q' * 33, 'Tool1234']:
        try:
            words.manage({'action': 'add_word', 'word': word})
            raise AssertionError('Duplicate or invalid word was accepted')
        except ValueError:
            pass
    literal = '\nEmail: demo+tag@example.com!\nhttps://example.com/a?x=1&y=2\n谢谢。\n'
    words.manage({'action': 'add_snippet', 'trigger': '我的联系信息', 'text': literal})
    assert words.expand('我的联系信息') == literal
    assert words.expand(' 我的联系信息 ') == literal
    for phrase in ['请发我的联系信息', '我的联系信息还有地址', '我的联系信息，']:
        assert words.expand(phrase) == phrase, 'A trigger inside another utterance must not expand'
    assert words.expand('我的联系信息', english=True) == '我的联系信息'
    assert Vocabulary(path).expand('我的联系信息') == literal
    for packet in [
        {'action': 'add_snippet', 'trigger': '我的联系信息', 'text': 'duplicate'},
        {'action': 'add_snippet', 'trigger': 'ab', 'text': 'short'},
        {'action': 'add_snippet', 'trigger': 'a' * 61, 'text': 'long'},
        {'action': 'add_snippet', 'trigger': 'new\ntrigger', 'text': 'line'},
        {'action': 'add_snippet', 'trigger': '我的联系信息。', 'text': 'punctuation'},
        {'action': 'add_snippet', 'trigger': 'New trigger', 'text': ''},
        {'action': 'add_snippet', 'trigger': 'New trigger', 'text': 'x' * 4001},
        {'action': 'add_snippet', 'trigger': 'New trigger', 'text': 'a\0b'},
    ]:
        try:
            words.manage(packet)
            raise AssertionError('Invalid or duplicate snippet was accepted')
        except ValueError:
            pass
    assert len(words.snapshot()['snippets']) == 1
    words.manage({'action': 'delete_word', 'word': '阿里云'})
    restored = Vocabulary(path)
    assert '阿里云' not in restored.counts and '阿里云' not in restored.confirmed
    assert restored.confirmed == ['QuietType'] and restored.expand('我的联系信息') == literal
    assert json.loads(path.read_text())['futureSetting'] is True

    # Manual additions and automatic learning cannot lose each other's updates.
    def learn():
        for _ in range(5):
            restored.record(['Qwen'], 'Qwen', 'Qwen')
    def add():
        for number in range(5):
            restored.manage({'action': 'add_snippet', 'trigger': f'测试短语{number}', 'text': f'内容{number}'})
    workers = [threading.Thread(target=learn), threading.Thread(target=add)]
    for worker in workers: worker.start()
    for worker in workers: worker.join()
    reloaded = Vocabulary(path)
    assert reloaded.counts['Qwen'] == 5 and len(reloaded.snippets) == 6
    reloaded.manage({'action': 'delete_snippet', 'trigger': '我的联系信息'})
    assert Vocabulary(path).expand('我的联系信息') == '我的联系信息'

    old = path.read_bytes()
    original_replace = server.os.replace
    try:
        server.os.replace = lambda *args: (_ for _ in ()).throw(OSError('disk unavailable'))
        try:
            reloaded.manage({'action': 'add_word', 'word': 'NewTool'})
            raise AssertionError('Failed save was reported as successful')
        except OSError:
            pass
        assert 'NewTool' not in reloaded.confirmed and path.read_bytes() == old
    finally:
        server.os.replace = original_replace

    # The personal HTTP route validates requests without starting a model/service.
    import io
    def request(method, body=None, origin=None, host='127.0.0.1:11436'):
        handler = object.__new__(server.Handler)
        data = json.dumps(body, ensure_ascii=False).encode() if body is not None else b''
        handler.path = '/personal'
        handler.headers = {'Host': host, 'Content-Length': str(len(data))}
        if origin: handler.headers['Origin'] = origin
        handler.rfile = io.BytesIO(data)
        handler.connection = SimpleNamespace(settimeout=lambda seconds: None)
        handler.server = SimpleNamespace(vocabulary=reloaded, server_port=11436)
        replies = []
        handler.send_json = lambda status, value: replies.append((status, value))
        getattr(handler, method)()
        return replies[0]
    assert request('do_GET')[0] == 200
    assert request('do_GET', origin='https://example.com')[0] == 403
    assert request('do_GET', host='example.com:11436')[0] == 403
    assert request('do_POST', {'action': 'add_word', 'word': 'NewTool'})[0] == 200
    assert request('do_POST', {'action': 'add_word', 'word': 'NewTool'})[0] == 400
    assert request('do_POST', ['bad schema'])[0] == 400
    assert request('do_POST', {'action': 'add_word', 'word': 'AnotherTool'}, origin='https://example.com')[0] == 403

    # Expansion happens only after cleanup and is never fed back through the editor.
    reloaded.manage({'action': 'add_snippet', 'trigger': '我的联系信息', 'text': literal})
    handler = object.__new__(server.Handler)
    pcm = np.full(16000, 1000, dtype='<i2').tobytes()
    handler.path = '/transcribe'
    handler.headers = {'Content-Length': str(len(pcm)), 'X-Session-Id': 'snippet-test'}
    handler.rfile = io.BytesIO(pcm)
    handler.connection = SimpleNamespace(settimeout=lambda seconds: None)
    handler.server = SimpleNamespace(vocabulary=reloaded, jobs={}, state_lock=threading.Lock(),
                                     inference_lock=threading.Lock(), model=object())
    replies, calls = [], []
    handler.send_json = lambda status, value: replies.append((status, value))
    originals = server.speech_audio, server.transcribe_audio, server.ensure_editor, server.refine_text
    try:
        server.speech_audio = lambda runtime, audio, cancelled: audio
        server.transcribe_audio = lambda *args: '我的联系信息'
        server.ensure_editor = lambda runtime: None
        def clean_once(text, english, cancelled, vocabulary, learned):
            calls.append(text)
            return '我的联系信息'
        server.refine_text = clean_once
        handler.do_POST()
        assert replies[0][0] == 200 and replies[0][1]['text'] == literal
        assert calls == ['我的联系信息']
    finally:
        server.speech_audio, server.transcribe_audio, server.ensure_editor, server.refine_text = originals

    # More confirmed words may be stored, while ASR hint limits stay unchanged.
    bounded = Vocabulary()
    bounded.confirmed = [f'Tool{i}' for i in range(MAX_WORDS)]
    assert len(bounded.hints()) <= MAX_HINTS and sum(map(len, bounded.hints())) <= 160
    try:
        bounded.manage({'action': 'add_word', 'word': 'ExtraTool'})
        raise AssertionError('Word limit was exceeded')
    except ValueError:
        pass
    bounded.snippets = [{'trigger': f'Trigger{i}', 'text': 'text'} for i in range(MAX_SNIPPETS)]
    try:
        bounded.manage({'action': 'add_snippet', 'trigger': 'Extra trigger', 'text': 'text'})
        raise AssertionError('Snippet limit was exceeded')
    except ValueError:
        pass
    path.write_text('{"counts": []}')
    malformed = Vocabulary(path)
    malformed.record(['NewTool'], 'NewTool', 'NewTool')
    assert path.read_text() == '{"counts": []}'
print('Personal word management, exact snippets, API validation, concurrent writes and literal output passed')

# Idle release shares the inference lock and must not overtake queued requests.
from unittest.mock import patch
runtime = SimpleNamespace(model=object(), vad_model=object(), warmed=True, last_inference=100,
                          inference_lock=threading.Lock(), state_lock=threading.Lock(), jobs={},
                          vocabulary=Vocabulary(), model_dir=Path('unused-test-model'))
with patch('mlx.core.clear_cache') as clear_cache, patch.object(server.time, 'monotonic', return_value=399):
    assert not server.unload_if_idle(runtime)
    assert runtime.model is not None and runtime.vad_model is not None
    assert not clear_cache.called
with patch('mlx.core.clear_cache') as clear_cache, patch.object(server.time, 'monotonic', return_value=400):
    runtime.inference_lock.acquire()
    assert not server.unload_if_idle(runtime)
    runtime.inference_lock.release()
    runtime.jobs['queued'] = threading.Event()
    assert not server.unload_if_idle(runtime)
    assert not clear_cache.called and runtime.model is not None
    runtime.jobs['queued'].set()
    assert server.unload_if_idle(runtime)
    assert runtime.model is None and runtime.vad_model is None and not runtime.warmed
    clear_cache.assert_called_once()
    assert not server.unload_if_idle(runtime)
    clear_cache.assert_called_once()
runtime.jobs.clear()

def idle_request(path, runtime):
    handler = object.__new__(server.Handler)
    data = np.full(16000, 1000, dtype='<i2').tobytes() if path == '/transcribe' else b''
    handler.path = path
    handler.headers = {'Content-Length': str(len(data)), 'X-Session-Id': 'idle-test'}
    handler.rfile = io.BytesIO(data)
    handler.connection = SimpleNamespace(settimeout=lambda seconds: None)
    handler.server = runtime
    replies = []
    handler.send_json = lambda status, value: replies.append((status, value))
    handler.do_POST()
    return replies[0]

# Warmup and later dictation both restore models and reset idle time before unlocking.
class WarmModel:
    def stream_generate(self, *args, **kwargs):
        yield None
def fake_speech(runtime, audio, cancelled):
    assert runtime.inference_lock.locked()
    if runtime.vad_model is None:
        runtime.vad_model = object()
    return audio
with patch.object(server, 'load_asr_model', return_value=WarmModel()) as load_model, \
        patch.object(server, 'speech_audio', side_effect=fake_speech), \
        patch.object(server, 'ensure_editor'), \
        patch.object(server, 'editor_request', return_value=io.StringIO('{}')) as editor, \
        patch.object(server.time, 'monotonic', return_value=500):
    assert idle_request('/warm', runtime)[0] == 200
    assert runtime.warmed and runtime.last_inference == 500
    assert not runtime.inference_lock.locked()
    assert editor.call_args.args[1]['keep_alive'] == '5m'
    load_model.assert_called_once()
with patch('mlx.core.clear_cache'), patch.object(server.time, 'monotonic', return_value=799):
    assert not server.unload_if_idle(runtime)
with patch('mlx.core.clear_cache'), patch.object(server.time, 'monotonic', return_value=800):
    assert server.unload_if_idle(runtime)
with patch.object(server, 'load_asr_model', return_value=WarmModel()) as load_model, \
        patch.object(server, 'speech_audio', side_effect=fake_speech), \
        patch.object(server, 'transcribe_audio', return_value='你好'), \
        patch.object(server, 'ensure_editor'), \
        patch.object(server, 'refine_text', return_value='你好'), \
        patch.object(server.time, 'monotonic', return_value=900):
    status, result = idle_request('/transcribe', runtime)
    assert status == 200 and result['text'] == '你好'
    assert runtime.model is not None and runtime.vad_model is not None
    assert runtime.last_inference == 900 and not runtime.jobs
    load_model.assert_called_once()

# Silence, errors and cancelled work cannot leave a stale idle timestamp or locked GPU.
for mode in ('silence', 'error', 'cancel'):
    def finish_with(runtime, audio, cancelled):
        if mode == 'silence':
            return None
        if mode == 'cancel':
            cancelled.set()
        raise ValueError('test interruption')
    with patch.object(server, 'speech_audio', side_effect=finish_with), \
            patch.object(server.time, 'monotonic', return_value=1000):
        status, result = idle_request('/transcribe', runtime)
        assert status == {'silence': 200, 'error': 500, 'cancel': 409}[mode]
        assert runtime.last_inference == 1000 and not runtime.jobs
        assert not runtime.inference_lock.locked()
    runtime.last_inference = 900
runtime.warmed = True
with patch.object(server, 'ensure_editor', side_effect=ValueError('test warmup failure')) as editor, \
        patch.object(server.time, 'monotonic', return_value=1100):
    assert idle_request('/warm', runtime)[0] == 500
    editor.assert_called_once()
    assert runtime.last_inference == 1100 and not runtime.inference_lock.locked()
print('Five-minute idle release, queued work protection, warmup, lazy reload and failure cleanup passed')
