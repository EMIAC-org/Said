#!/usr/bin/env python3
"""Polish prompt benchmark: run each prompt in prompts/ against the same cases on
DeepInfra Gemma, with the request the control-plane sends, and score them.

  python3 lab/polish_prompt_bench/bench.py                  # all prompts, cases + your corpus
  python3 lab/polish_prompt_bench/bench.py --only v2,v3     # some prompts
  python3 lab/polish_prompt_bench/bench.py --repeat 2       # run twice, count answers that change

Two test sets:
  cases.jsonl  hand-written cases, each with strings the reply must / must not contain.
  corpus       your own past dictations from lab/corpus (raw Whisper → the text you kept),
               scored by how many words the reply is away from what you kept. Not in git.

Results are cached in out/cache.jsonl, so editing one prompt re-runs only that prompt.
The report lands in out/<time>/report.md with every reply side by side.
"""

import argparse
import concurrent.futures as cf
import datetime as dt
import difflib
import glob
import hashlib
import json
import os
import re
import statistics
import threading
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
OUT = os.path.join(HERE, "out")
ENDPOINT = "https://api.deepinfra.com/v1/openai/chat/completions"
MODEL = "google/gemma-4-26B-A4B-it"

# The same language lines as said_core::polish::dictation.
LANGUAGE = {
    "english": ("Write the result in English: translate Hindi words into English.",
                "Change, add, drop or reorder any other word."),
    "hindi": ("Write Hindi words in Devanagari script. Keep English words in English.",
              "Change, add, drop, reorder or translate any other word."),
    "hinglish": ("Keep the speaker's mix of Hindi and English. Write Hindi words in Roman letters, never in Devanagari.",
                 "Change, add, drop, reorder or translate any other word."),
}
INFO_CATS = {"grammar", "self_correction", "risky_dictionary"}
FILLERS = {"um", "umm", "uh", "uhh", "hmm", "hm", "er", "ah"}
# Spoken forms that legitimately collapse when written ("at the rate gmail dot com" → "@gmail.com").
SPOKEN_FORMS = {"dot", "slash", "at", "rate", "colon", "percent", "pm", "am", "zero", "one", "two", "three",
                "four", "five", "six", "seven", "eight", "nine", "ten", "twenty", "thirty", "forty", "fifty",
                "hundred", "thousand", "sau", "do"}
DEVANAGARI = re.compile(r"[ऀ-ॿ]")


# ── prompts ──────────────────────────────────────────────────────────────────

def load_prompts():
    prompts = {}
    for path in sorted(glob.glob(os.path.join(HERE, "prompts", "*"))):
        name = os.path.basename(path)
        if os.path.isdir(path):
            prompts[name] = {"kind": "old", "dir": path}
        elif name.endswith(".txt"):
            prompts[name[:-4]] = {"kind": "tagged", "system": open(path).read().strip()}
    return prompts


def word_list(words):
    lines = [f"- {h} → {w}" if h else f"- {w}" for h, w in words or []]
    if not lines:
        return ""
    return "Word list (where the transcript has the left side, write the right side):\n" + "\n".join(lines) + "\n\n"


def request_body(prompt, text, lang, words):
    n = max(1, len(text.split()))
    if prompt["kind"] == "old":
        system = open(os.path.join(prompt["dir"], f"system.{lang}.txt")).read()
        user = open(os.path.join(prompt["dir"], f"user.{lang}.txt")).read().replace("{{TRANSCRIPT}}", text.strip())
        # The old server sent vocabulary inside its system prompt; here it gets the same word list as the rest.
        user = word_list(words) + user
        stop = ["=== BEGIN TRANSCRIPT", "=== END TRANSCRIPT", "<transcript>", "</transcript>"]
        max_tokens = min(1024, max(128, n * 2 + 64))
    else:
        language, keep_words = LANGUAGE[lang]
        system = prompt["system"].replace("{language}", language).replace("{keep_words}", keep_words)
        user = f"{word_list(words)}<transcript>\n{text.strip()}\n</transcript>"
        stop = ["</transcript>"]
        max_tokens = min(2048, max(128, n * 3 + 64))
    return {
        "model": MODEL, "temperature": 0.0, "top_p": 0.9, "max_tokens": max_tokens,
        "stream": False, "service_tier": "priority", "stop": stop,
        "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
    }


# ── calling the model, with a cache ──────────────────────────────────────────

def api_key():
    if os.environ.get("DEEPINFRA_API_KEY"):
        return os.environ["DEEPINFRA_API_KEY"]
    for line in open(os.path.join(REPO, ".env")):
        if line.startswith("DEEPINFRA_API_KEY="):
            return line.split("=", 1)[1].strip().strip('"')
    raise SystemExit("set DEEPINFRA_API_KEY")


class Cache:
    def __init__(self, path):
        self.path, self.lock, self.rows = path, threading.Lock(), {}
        if os.path.exists(path):
            for line in open(path):
                row = json.loads(line)
                self.rows[row["key"]] = row

    def get(self, key):
        return self.rows.get(key)

    def put(self, row):
        with self.lock:
            self.rows[row["key"]] = row
            with open(self.path, "a") as f:
                f.write(json.dumps(row, ensure_ascii=False) + "\n")


def call(body, key, cache, repeat):
    ck = hashlib.sha256((json.dumps(body, sort_keys=True) + f"#{repeat}").encode()).hexdigest()
    hit = cache.get(ck)
    if hit:
        return hit["reply"], hit["ms"]
    req = urllib.request.Request(ENDPOINT, data=json.dumps(body).encode(), headers={
        "Authorization": f"Bearer {key}", "Content-Type": "application/json"})
    for attempt in range(4):
        try:
            started = time.time()
            with urllib.request.urlopen(req, timeout=60) as resp:
                reply = json.load(resp)["choices"][0]["message"]["content"] or ""
            ms = int((time.time() - started) * 1000)
            cache.put({"key": ck, "reply": reply, "ms": ms})
            return reply, ms
        except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError) as err:
            if attempt == 3:
                return f"<<ERROR {err}>>", 0
            time.sleep(2 * (attempt + 1))


# ── scoring ──────────────────────────────────────────────────────────────────

def norm(text):
    """Lower case without the punctuation a cleaner may add: "Theek hai, main" matches "theek hai main"."""
    text = text.lower().replace("’", "'")
    text = re.sub(r"[,;!?\"“”()]", " ", text)
    text = re.sub(r"[.:](?=\s|$)", " ", text)
    return " ".join(text.split())


def has_word(text, term):
    if re.fullmatch(r"[\w' ]+", term):
        return re.search(r"(?<!\w)" + re.escape(term) + r"(?!\w)", text, re.I) is not None
    return term.lower() in text.lower()


def check_case(case, reply):
    fails = []
    text = norm(reply)
    for m in case.get("must", []):
        if norm(m) not in text:
            fails.append(f"missing “{m}”")
    for group in case.get("must_any", []):
        if not any(norm(m) in text for m in group):
            fails.append(f"missing one of {group}")
    for m in case.get("must_not", []):
        if has_word(text, m):
            fails.append(f"has “{m}”")
    for pattern in case.get("must_re", []):
        if not re.search(pattern, reply.strip(), re.I):
            fails.append(f"shape /{pattern}/")
    return fails


def content(text):
    """Words that carry meaning: no fillers, no punctuation, emails and links split into their parts."""
    return [t for t in re.findall(r"[\w']+", text.lower().replace("’", "'")) if t not in FILLERS]


def guards(source, reply, lang):
    """Faults any reply can have, whatever the case asked for."""
    faults = []
    stripped = reply.strip()
    if not stripped or stripped.startswith("<<ERROR"):
        return ["empty or error"]
    if lang != "hindi" and DEVANAGARI.search(stripped):
        faults.append("Devanagari")
    if re.match(r"^(here|output|cleaned|sure|reply)\b", stripped, re.I) or "<transcript" in stripped or "===" in stripped:
        faults.append("wrapper text")
    if stripped[0] in "\"'“" and stripped[-1] in "\"'”":
        faults.append("quoted")
    src = [t for t in content(source) if t not in SPOKEN_FORMS]
    out = [t for t in content(stripped) if t not in SPOKEN_FORMS and not t.isdigit()]
    if lang != "english" and not DEVANAGARI.search(source):
        lost, added = lost_and_added(source, stripped)
        faults += [f"lost “{x}”" for x in lost] + [f"added “{x}”" for x in added]
    if len(src) >= 5 and lang != "english" and not DEVANAGARI.search(source):
        ratio = len(out) / len(src)
        if ratio < 0.7:
            faults.append(f"dropped words ({len(out)}/{len(src)})")
        elif ratio > 1.3:
            faults.append(f"added words ({len(out)}/{len(src)})")
    return faults


def lost_and_added(source, reply):
    """Runs of 3+ spoken words missing from the reply, and runs of 3+ words the speaker never said.
    Catches a cleaner that quietly drops a clause from a long dictation, which counts don't."""
    src = [t for t in content(source) if t not in SPOKEN_FORMS]
    out = [t for t in content(reply) if t not in SPOKEN_FORMS and not t.isdigit()]
    lost, added = [], []
    for op, i1, i2, j1, j2 in difflib.SequenceMatcher(None, src, out, autojunk=False).get_opcodes():
        if op in ("delete", "replace") and (i2 - i1) - (j2 - j1) >= 3:
            lost.append(" ".join(src[i1:i2]))
        if op in ("insert", "replace") and (j2 - j1) - (i2 - i1) >= 3:
            added.append(" ".join(out[j1:j2]))
    return lost, added


def word_distance(a, b):
    a, b = content(a), content(b)
    prev = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        cur = [i]
        for j, y in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (x != y)))
        prev = cur
    return prev[-1] / max(1, len(b))


# ── test sets ────────────────────────────────────────────────────────────────

def load_cases():
    rows = []
    for line in open(os.path.join(HERE, "cases.jsonl")):
        if line.strip() and not line.startswith("#"):
            rows.append(json.loads(line))
    return rows


def load_corpus(max_change):
    """Your past dictations where what you kept is a light edit of what Whisper heard."""
    paths = sorted(glob.glob(os.path.join(REPO, "lab", "corpus", "learning_corpus_local_*.jsonl")))
    if not paths:
        return []
    seen, rows = set(), []
    for line in open(paths[-1]):
        r = json.loads(line)
        raw, kept = (r.get("raw_stt") or "").strip(), (r.get("user_kept") or "").strip()
        if not raw or not kept or raw in seen:
            continue
        seen.add(raw)
        if word_distance(raw, kept) <= max_change and len(content(raw)) >= 3:
            rows.append({"id": r.get("sample_id", "")[-8:], "input": raw, "kept": kept, "lang": "hinglish"})
    return rows


# ── run ──────────────────────────────────────────────────────────────────────

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", help="comma list of prompt name prefixes, e.g. v0,v2")
    ap.add_argument("--repeat", type=int, default=1)
    ap.add_argument("--no-corpus", action="store_true")
    ap.add_argument("--corpus-max-change", type=float, default=0.3)
    ap.add_argument("--workers", type=int, default=10)
    args = ap.parse_args()

    prompts = load_prompts()
    if args.only:
        wanted = [p.strip() for p in args.only.split(",")]
        prompts = {k: v for k, v in prompts.items() if any(k.startswith(w) for w in wanted)}
    cases = load_cases()
    corpus = [] if args.no_corpus else load_corpus(args.corpus_max_change)
    key = api_key()
    os.makedirs(OUT, exist_ok=True)
    cache = Cache(os.path.join(OUT, "cache.jsonl"))

    jobs = []
    for name, prompt in prompts.items():
        for rep in range(args.repeat):
            for case in cases:
                jobs.append((name, "case", case, rep, request_body(prompt, case["input"], case["lang"], case.get("words"))))
            if rep == 0:
                for row in corpus:
                    jobs.append((name, "corpus", row, 0, request_body(prompt, row["input"], row["lang"], None)))
    print(f"{len(prompts)} prompts × {len(cases)} cases × {args.repeat} + {len(corpus)} corpus rows = {len(jobs)} calls")

    results = {}
    done = [0]
    def run(job):
        name, kind, item, rep, body = job
        reply, ms = call(body, key, cache, rep)
        results[(name, kind, item["id"], rep)] = (reply.strip(), ms)
        done[0] += 1
        if done[0] % 50 == 0:
            print(f"  {done[0]}/{len(jobs)}", flush=True)
    with cf.ThreadPoolExecutor(args.workers) as pool:
        list(pool.map(run, jobs))

    report(prompts, cases, corpus, results, args.repeat)


def report(prompts, cases, corpus, results, repeats):
    stamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    folder = os.path.join(OUT, stamp)
    os.makedirs(folder, exist_ok=True)
    names = list(prompts)
    core = [c for c in cases if c["cat"] not in INFO_CATS]
    cats = sorted({c["cat"] for c in cases}, key=lambda c: (c in INFO_CATS, c))
    lines = [f"# Polish prompt benchmark — {stamp}", "",
             f"Model `{MODEL}` on DeepInfra, temperature 0, the control-plane's request. "
             f"{len(cases)} cases ({len(core)} scored, {len(cases) - len(core)} behaviour checks), "
             f"{len(corpus)} of your past dictations, {repeats} run(s).", ""]

    summary = []
    for n in names:
        passed = sum(not check_case(c, results[(n, "case", c["id"], 0)][0]) for c in core)
        faults = sum(bool(guards(c["input"], results[(n, "case", c["id"], 0)][0], c["lang"])) for c in cases)
        faults += sum(bool(guards(r["input"], results[(n, "corpus", r["id"], 0)][0], "hinglish")) for r in corpus)
        dist = statistics.mean(word_distance(results[(n, "corpus", r["id"], 0)][0], r["kept"]) for r in corpus) if corpus else float("nan")
        ms = sorted(results[(n, "case", c["id"], 0)][1] for c in cases)
        flips = sum(len({results[(n, "case", c["id"], r)][0] for r in range(repeats)}) > 1 for c in cases) if repeats > 1 else None
        summary.append((n, passed, faults, dist, ms[len(ms) // 2], ms[int(len(ms) * 0.9)], flips))
    raw_dist = statistics.mean(word_distance(r["input"], r["kept"]) for r in corpus) if corpus else float("nan")

    lines += ["## Score", "",
              "| prompt | cases passed | faults (all replies) | words away from what you kept | p50 / p90 ms |" + (" changed on re-run |" if repeats > 1 else ""),
              "|---|---|---|---|---|" + ("---|" if repeats > 1 else "")]
    for n, passed, faults, dist, p50, p90, flips in summary:
        lines.append(f"| {n} | {passed}/{len(core)} | {faults} | {dist:.1%} | {p50} / {p90} |" + (f" {flips} |" if repeats > 1 else ""))
    if corpus:
        lines.append(f"| *(raw Whisper, no polish)* | — | — | {raw_dist:.1%} | — |")
    lines += ["", "Lower “words away” is better: the share of words that differ from the text you actually kept.", ""]

    lines += ["## By kind of case", "", "| kind | " + " | ".join(names) + " |", "|---|" + "---|" * len(names)]
    for cat in cats:
        group = [c for c in cases if c["cat"] == cat]
        cells = [f"{sum(not check_case(c, results[(n, 'case', c['id'], 0)][0]) for c in group)}/{len(group)}" for n in names]
        label = f"*{cat}* (behaviour)" if cat in INFO_CATS else cat
        lines.append(f"| {label} | " + " | ".join(cells) + " |")
    lines += ["", "Behaviour rows are choices, not scores: grammar = fixed the grammar, self_correction = kept only the "
              "corrected value, risky_dictionary = left a common word alone despite a bad Dictionary entry.", ""]

    lines += ["## Every case", ""]
    for c in cases:
        lines.append(f"### {c['id']} · {c['cat']}" + (f" · {c['lang']}" if c["lang"] != "hinglish" else ""))
        lines.append(f"`{c['input']}`" + (f"  \nword list: {c['words']}" if c.get("words") else "") + (f"  \n_{c['note']}_" if c.get("note") else ""))
        lines.append("")
        for n in names:
            reply = results[(n, "case", c["id"], 0)][0]
            fails = check_case(c, reply) + guards(c["input"], reply, c["lang"])
            mark = "✓" if not fails else "✗ " + "; ".join(fails)
            lines.append(f"- **{n}** {mark}  \n  {reply}")
        lines.append("")

    if corpus:
        lines += ["## Your dictations where the prompts disagree most", ""]
        spread = []
        for r in corpus:
            d = [word_distance(results[(n, "corpus", r["id"], 0)][0], r["kept"]) for n in names]
            spread.append((max(d) - min(d), r, d))
        for _, r, d in sorted(spread, key=lambda x: -x[0])[:25]:
            lines.append(f"**Whisper:** {r['input']}  \n**You kept:** {r['kept']}")
            for n, dist in zip(names, d):
                reply = results[(n, "corpus", r["id"], 0)][0]
                faults = guards(r["input"], reply, "hinglish")
                lines.append(f"- **{n}** ({dist:.0%} away{'; ' + '; '.join(faults) if faults else ''})  \n  {reply}")
            lines.append("")

    path = os.path.join(folder, "report.md")
    open(path, "w").write("\n".join(lines) + "\n")
    with open(os.path.join(folder, "results.jsonl"), "w") as f:
        for (n, kind, item_id, rep), (reply, ms) in results.items():
            f.write(json.dumps({"prompt": n, "set": kind, "id": item_id, "run": rep, "reply": reply, "ms": ms}, ensure_ascii=False) + "\n")
    print("\n".join(lines[: lines.index("## Every case")]))
    print(f"report: {path}")


if __name__ == "__main__":
    main()
