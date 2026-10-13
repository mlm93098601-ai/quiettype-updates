"""Small, per-user spelling hints. Never replace a transcript with a guess."""
import json
import os
import re
import threading
from pathlib import Path

MAX_WORDS = 200
MAX_HINTS = 12
MIN_USES = 3
MAX_SNIPPETS = 50
MAX_STORE_BYTES = 1024 * 1024
COMMON = set("现在 今天 明天 这个 那个 东西 事情 问题 软件 模型 文件 输入框 用户 文字 测试 test file software model today hello".split())


def valid_word(word):
    if not isinstance(word, str):
        return False
    return (2 <= len(word) <= 32 and word == word.strip() and word.lower() not in COMMON
            and re.fullmatch(r"[^\W\d_][\w .+\-]*", word) is not None
            and re.search(r"\d{4}", word) is None)


def spelling(text):
    return "".join(c for c in text.casefold() if c.isalnum())


def contains(word, text):
    if word.isascii():
        return re.search(r"(?<![A-Za-z0-9])" + re.escape(word) + r"(?![A-Za-z0-9])", text, re.I) is not None
    return word in text


def valid_trigger(trigger):
    return (isinstance(trigger, str) and 3 <= len(trigger) <= 60
            and trigger == " ".join(trigger.split()) and any(c.isalnum() for c in trigger)
            and all(c.isalnum() or c == " " for c in trigger))


def valid_expansion(text):
    return (isinstance(text, str) and 1 <= len(text) <= 4000 and bool(text.strip())
            and not any((ord(c) < 32 and c not in "\n\r\t") or ord(c) == 127 for c in text))


class Vocabulary:
    def __init__(self, path=None):
        self.path = Path(path) if path else None
        self.lock = threading.RLock()
        self.counts, self.confirmed = {}, []
        self.snippets, self.extra, self.load_error = [], {}, None
        if self.path and self.path.is_file():
            try:
                if self.path.stat().st_size > MAX_STORE_BYTES:
                    raise ValueError("Personal vocabulary is too large")
                data = json.loads(self.path.read_text())
                if (not isinstance(data, dict) or not isinstance(data.get("counts", {}), dict)
                        or not isinstance(data.get("confirmed", []), list)
                        or not isinstance(data.get("snippets", []), list)):
                    raise ValueError("Invalid personal vocabulary")
                self.counts = {w: min(n, 1_000_000) for w, n in data.get("counts", {}).items()
                               if valid_word(w) and type(n) is int and n > 0}
                self.counts = dict(sorted(self.counts.items(), key=lambda p: p[1], reverse=True)[:MAX_WORDS])
                self.confirmed = list(dict.fromkeys(w for w in data.get("confirmed", []) if valid_word(w)))[:MAX_WORDS]
                seen = set()
                for snippet in data.get("snippets", []):
                    if (not isinstance(snippet, dict) or not valid_trigger(snippet.get("trigger"))
                            or not valid_expansion(snippet.get("text"))
                            or snippet["trigger"].casefold() in seen or len(self.snippets) >= MAX_SNIPPETS):
                        raise ValueError("Invalid voice snippet")
                    seen.add(snippet["trigger"].casefold())
                    self.snippets.append({"trigger": snippet["trigger"], "text": snippet["text"]})
                self.extra = data
            except (OSError, ValueError, TypeError, AttributeError):
                self.counts, self.confirmed = {}, []
                self.snippets = []
                self.load_error = "个人词库文件无法读取，原文件已保留，请联系开发者修复"

    def save(self):
        if self.load_error:
            raise ValueError(self.load_error)
        if self.path:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            temporary = self.path.with_suffix(".tmp")
            data = dict(self.extra, version=2, counts=self.counts, confirmed=self.confirmed, snippets=self.snippets)
            try:
                temporary.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
                os.replace(temporary, self.path)
            finally:
                temporary.unlink(missing_ok=True)

    def snapshot(self):
        with self.lock:
            if self.load_error:
                raise ValueError(self.load_error)
            confirmed = {spelling(word) for word in self.confirmed}
            words = list(dict.fromkeys(self.confirmed + sorted(self.counts, key=self.counts.get, reverse=True)))
            return {"words": [{"word": word, "count": self.counts.get(word, 0),
                               "confirmed": spelling(word) in confirmed} for word in words],
                    "snippets": [dict(snippet) for snippet in self.snippets]}

    def manage(self, packet):
        with self.lock:
            if self.load_error:
                raise ValueError(self.load_error)
            if not isinstance(packet, dict):
                raise ValueError("个人词库操作格式不正确")
            action = packet.get("action")
            previous = self.counts.copy(), self.confirmed[:], self.snippets[:]
            try:
                if action in ("add_word", "delete_word"):
                    word = packet.get("word")
                    if not valid_word(word):
                        raise ValueError("词语需为 2–32 个字符，以文字开头，不含四位连续数字或普通停顿词")
                    key = spelling(word)
                    if action == "add_word":
                        if any(spelling(w) == key for w in self.confirmed):
                            raise ValueError("这个词语已在确认词库中")
                        if len(self.confirmed) >= MAX_WORDS:
                            raise ValueError("确认词库最多保存 200 个词语")
                        self.confirmed.append(next((w for w in self.counts if spelling(w) == key), word))
                    else:
                        self.counts = {w: n for w, n in self.counts.items() if spelling(w) != key}
                        self.confirmed = [w for w in self.confirmed if spelling(w) != key]
                elif action in ("add_snippet", "delete_snippet"):
                    trigger = packet.get("trigger")
                    if not valid_trigger(trigger):
                        raise ValueError("触发词需为 3–60 个文字、数字或空格，不含标点、换行或多余空格")
                    key = trigger.casefold()
                    if action == "add_snippet":
                        if any(s["trigger"].casefold() == key for s in self.snippets):
                            raise ValueError("这个触发词已存在，请先删除原短语")
                        if not valid_expansion(packet.get("text")):
                            raise ValueError("展开内容需为 1–4000 个字符且不能为空")
                        if len(self.snippets) >= MAX_SNIPPETS:
                            raise ValueError("语音短语最多保存 50 条")
                        self.snippets.append({"trigger": trigger, "text": packet["text"]})
                    else:
                        self.snippets = [s for s in self.snippets if s["trigger"].casefold() != key]
                else:
                    raise ValueError("不支持的个人词库操作")
                self.save()
            except (OSError, ValueError):
                self.counts, self.confirmed, self.snippets = previous
                raise
            return self.snapshot()

    def expand(self, text, english=False):
        # shortcut: exact whole cleaned utterances only; add partial matching only with explicit user control.
        with self.lock:
            if not english:
                for snippet in self.snippets:
                    if text.strip().casefold() == snippet["trigger"].casefold():
                        return snippet["text"]
        return text

    def hints(self):
        with self.lock:
            return self._hints()

    def _hints(self):
        # Three separate utterances before an automatically learned word is used.
        frequent = [w for w, n in sorted(self.counts.items(), key=lambda p: p[1], reverse=True) if n >= MIN_USES]
        result, seen, size = [], set(), 0
        for word in self.confirmed + frequent:
            if spelling(word) in seen or size + len(word) > 160:
                continue
            result.append(word); seen.add(spelling(word)); size += len(word)
            if len(result) == MAX_HINTS:
                break
        return result

    def prompt(self):
        words = self.hints()
        return ("以下词语仅供拼写参考，音频优先，没有说出的词语不要输出：" + ", ".join(words)) if words else None

    def record(self, candidates, raw, output):
        with self.lock:
            self._record(candidates, raw, output)

    def _record(self, candidates, raw, output):
        # Only names present in both ASR and the accepted edit can be learned.
        # No raw transcript, audio, prompt, phone number or account is persisted.
        if self.load_error or not isinstance(candidates, list):
            return
        if re.search(r"密码|验证码|账号|帐号|手机号码|电话号码|身份证|密钥|\b(?:password|passcode|secret|token|api[ -]?key|account)\b", raw, re.I):
            return
        seen = set()
        for word in candidates[:8]:
            if not valid_word(word):
                continue
            key = spelling(word)
            if key in seen or not contains(word, raw) or not contains(word, output):
                continue
            seen.add(key)
            word = next((w for w in self.counts if spelling(w) == key), word)
            self.counts[word] = min(self.counts.get(word, 0) + 1, 1_000_000)
        self.counts = dict(sorted(self.counts.items(), key=lambda p: p[1], reverse=True)[:MAX_WORDS])
        if self.path and seen:
            try:
                self.save()
            except OSError:
                # Dictionary persistence must never prevent dictation.
                print("Local vocabulary could not be saved", flush=True)

    def protects(self, raw, output):
        # Protect known spellings and explicit English identifiers already heard.
        words = self.hints() + re.findall(r"\b(?=[A-Za-z0-9.+\-]*[A-Z])[A-Za-z][A-Za-z0-9.+\-]{1,31}\b", raw)
        before, after = spelling(raw), spelling(output)
        return all(spelling(w) not in before or spelling(w) in after for w in words)
