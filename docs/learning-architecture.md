# Said — Personalized Learning Architecture
## From Prompt Hacks to a Self-Improving Voice Engine

---

## Part 1: The AI Family Tree (What's What)

```
                              ARTIFICIAL INTELLIGENCE
                          "machines that act intelligently"
                                       │
                    ┌──────────────────┼──────────────────┐
                    │                  │                  │
              Rule-Based          MACHINE LEARNING     Expert Systems
           (if-else logic)    "learns from data"      (knowledge DBs)
                                       │
                    ┌──────────────────┼──────────────────┐
                    │                  │                  │
              Classical ML      DEEP LEARNING      Probabilistic
            (SVM, trees,      "neural networks"    (Bayesian, HMM)
             k-NN, BM25)            │
                    ┌───────────────┼───────────────┐
                    │               │               │
              Supervised      Unsupervised    REINFORCEMENT
            "here's the       "find patterns"   LEARNING
             answer, learn"   (clustering,     "learn from
             (classification,  embeddings)      trial & error"
              regression)           │               │
                    │               │          ┌────┼────┐
                    │               │          │         │
                    │               │        RLHF      DPO
                    │               │    "human ranks  "learn from
                    │               │     outputs,     preference
                    │               │     train        pairs
                    │               │     reward       directly"
                    │               │     model"
                    │               │
                    └───────┬───────┘
                            │
                     WHAT SAID USES
```

### Where Said Currently Sits

```
┌─────────────────────────────────────────────────────────────┐
│                    SAID's TECH MAP                          │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ✅ Rule-Based:    STT alias substitution (meac → EMIAC)   │
│  ✅ Classical ML:  BM25 text search, phonetic matching      │
│  ✅ Deep Learning: Groq LLM (Llama 70B) for polish         │
│  ✅ Unsupervised:  Gemini embeddings for RAG similarity     │
│  ❌ Supervised:    NOT YET — no per-user trained model      │
│  ❌ RL/DPO:        NOT YET — corrections collected but      │
│                    not used for model training               │
│                                                             │
│  KEY INSIGHT: Said collects DPO training data already       │
│  (edit_events = preference pairs) but throws it away        │
│  after extracting simple rules from it.                     │
└─────────────────────────────────────────────────────────────┘
```

---

## Part 2: Said's Current Pipeline (What Happens When You Speak)

```
 YOU SPEAK            LOCAL_SPEECH              SAID BACKEND              YOUR SCREEN
 ═════════          ═══════════           ══════════════             ════════════

 "EMIAC mein    ──►  WebSocket   ──►   ┌──────────────────┐
  bahut kaam         STT               │  1. PRE-LLM SUB  │
  hai aaj"                             │  meac → EMIAC     │──► "EMIAC mein bahut..."
                  "meac mein           │  (exact aliases)  │
                   bahut kaam          ├──────────────────┤
                   hai aaj"            │  2. VOCAB SELECT  │
                                       │  Pick relevant    │
                  (garbled!)           │  terms for prompt │
                                       ├──────────────────┤
                                       │  3. LLM POLISH   │
                                       │  Groq Llama 70B  │──► "EMIAC mein bahut
                                       │  + vocab hints   │     kaam hai aaj."
                                       │  + RAG examples  │
                                       ├──────────────────┤
                                       │  4. ROMANIZER    │
                                       │  Devanagari →    │──► (ensures Roman
                                       │  Roman script    │     Hinglish output)
                                       └──────────────────┘
                                              │
                                              ▼
                                       ┌──────────────┐        ┌──────────────┐
                                       │ PASTE TO APP │──────► │ EDIT WATCH   │
                                       │ (HID typing) │        │ Did user     │
                                       └──────────────┘        │ change it?   │
                                                               └──────┬───────┘
                                                                      │
                                                          ┌───────────┼───────────┐
                                                          │           │           │
                                                       ACCEPTED   CORRECTED   DELETED
                                                       (no-op)   (learn!)    (bad output)
                                                                     │
                                                                     ▼
                                                              ┌──────────────┐
                                                              │ STORE AS:    │
                                                              │ • STT alias  │
                                                              │ • Correction │
                                                              │ • RAG pair   │
                                                              │ • Vocab term │
                                                              └──────────────┘
```

### The Ceiling Problem

```
  WHAT WORKS                          WHAT DOESN'T
  ══════════                          ══════════════

  "meac" → EMIAC  ✅                  "meah" → ???  ❌
  (exact alias match)                 (never seen before)

  "mecobs" → Macobs  ✅              "mvac" → ???  ❌
  (exact alias match)                 (novel distortion)

  "aneten" → n8n  ✅                 "yarmiac" → ???  ❌
  (exact alias match)                 (creative Local speech garble)


  ┌─────────────────────────────────────────────────────┐
  │                                                     │
  │   CURRENT SUCCESS RATE: ~81%                        │
  │                                                     │
  │   WHY: The LLM (Scout 17B) follows EXACT alias     │
  │   hints but CANNOT do phonetic reasoning about      │
  │   distortions it hasn't seen before.                │
  │                                                     │
  │   CEILING: No matter how good the prompt is,        │
  │   a cloud LLM won't learn YOUR accent patterns.     │
  │   Every user is different. Every microphone is       │
  │   different. Local speech's failures are user-specific.  │
  │                                                     │
  │   TARGET: 98%+                                      │
  │                                                     │
  └─────────────────────────────────────────────────────┘
```

---

## Part 3: Key Concepts (Each One Mapped to Said)

### 3.1 Supervised Learning (SFT — Supervised Fine-Tuning)

```
  CONCEPT                              SAID'S VERSION
  ═══════                              ══════════════

  Training data:                       Said's edit_events table:

  ┌──────────────────────┐            ┌──────────────────────────────┐
  │ Input    │ Label     │            │ transcript    │ user_kept    │
  ├──────────┼───────────┤            ├───────────────┼──────────────┤
  │ cat photo│ "cat"     │            │ meac mein     │ EMIAC mein   │
  │ dog photo│ "dog"     │            │ mvac ka kaam  │ EMIAC ka kaam│
  │ car photo│ "car"     │            │ mecobs aur    │ Macobs aur   │
  └──────────┴───────────┘            └───────────────┴──────────────┘

  "Show me the answer,                "Show me what the user actually
   I'll learn the pattern"             wanted, I'll learn to produce it"

  HOW IT WORKS:
  ┌────────────────────────────────────────────────────────────────┐
  │                                                                │
  │  1. Collect (input, correct_output) pairs                      │
  │  2. Feed them to a neural network                              │
  │  3. Network adjusts its weights to minimize the difference     │
  │     between its output and the correct output                  │
  │  4. After many examples, network generalizes to new inputs     │
  │                                                                │
  │  ANALOGY: Teaching a child by showing flash cards.             │
  │  "This is a cat. This is a dog." After 100 cards, the child   │
  │  can recognize animals it's never seen before.                 │
  │                                                                │
  └────────────────────────────────────────────────────────────────┘
```

### 3.2 Reinforcement Learning (RL) & RLHF

```
  CONCEPT                              SAID'S VERSION
  ═══════                              ══════════════

  ┌─────────────────────────────────────────────────────────────┐
  │                                                             │
  │  REINFORCEMENT LEARNING:                                    │
  │                                                             │
  │  Agent ──► takes Action ──► gets Reward/Punishment          │
  │    │                              │                         │
  │    └──────── learns from ◄────────┘                         │
  │                                                             │
  │  RLHF (RL from Human Feedback):                             │
  │                                                             │
  │  LLM generates 2 outputs ──► Human picks the better one    │
  │    │                              │                         │
  │    │                    Train a "reward model" that          │
  │    │                    predicts which output humans prefer  │
  │    │                              │                         │
  │    └── fine-tune LLM to maximize reward model's score       │
  │                                                             │
  └─────────────────────────────────────────────────────────────┘

  FOR SAID:

  ┌──────────────────────────────────────────────────────────┐
  │                                                          │
  │  Agent    = Said's polish pipeline                       │
  │  Action   = output text ("EMIAC mein kaam hai")          │
  │  Reward   = user ACCEPTED the output (didn't edit)       │
  │  Punish   = user CORRECTED or DELETED the output         │
  │                                                          │
  │  Said already has the reward signal!                     │
  │  The edit-watch system IS the human feedback loop:       │
  │                                                          │
  │    accepted = +1 reward                                  │
  │    corrected = -1 reward + learning signal               │
  │    deleted = -2 reward (output was garbage)               │
  │                                                          │
  └──────────────────────────────────────────────────────────┘
```

### 3.3 DPO (Direct Preference Optimization) — The Key Technique

```
  WHY DPO OVER RLHF?
  ═══════════════════

  RLHF (complex, 3 steps):
  ┌──────────────────────────────────────────────────────────┐
  │  Step 1: Collect human preferences                       │
  │  Step 2: Train a reward model on preferences             │
  │  Step 3: Use RL (PPO) to fine-tune LLM against reward    │
  │                                                          │
  │  Problems: unstable training, reward hacking,            │
  │  needs lots of compute, hard to debug                    │
  └──────────────────────────────────────────────────────────┘

  DPO (simple, 1 step):
  ┌──────────────────────────────────────────────────────────┐
  │  Step 1: Collect preference pairs (chosen vs rejected)   │
  │  Step 2: Fine-tune LLM directly on the pairs             │
  │                                                          │
  │  No reward model needed! No RL instability!              │
  │  Just: "this output was preferred over that output"      │
  └──────────────────────────────────────────────────────────┘

  SAID'S DPO DATA (already in the database!):
  ┌────────────────────────────────────────────────────────────────┐
  │                                                                │
  │  edit_events table:                                            │
  │                                                                │
  │  ┌─────────────┬──────────────────┬──────────────────┐        │
  │  │ transcript   │ ai_output        │ user_kept        │        │
  │  │ (input)      │ (REJECTED)       │ (CHOSEN)         │        │
  │  ├─────────────┼──────────────────┼──────────────────┤        │
  │  │ meac mein   │ Meac mein kaam   │ EMIAC mein kaam  │        │
  │  │ kaam hai    │ hai aaj.         │ hai aaj.         │        │
  │  ├─────────────┼──────────────────┼──────────────────┤        │
  │  │ mvac ka     │ Mvac ka project  │ EMIAC ka project │        │
  │  │ project     │ ready hai.       │ ready hai.       │        │
  │  ├─────────────┼──────────────────┼──────────────────┤        │
  │  │ mecobs aur  │ Mecobs aur n8n   │ Macobs aur n8n   │        │
  │  │ aneten      │ pe kaam karo.    │ pe kaam karo.    │        │
  │  └─────────────┴──────────────────┴──────────────────┘        │
  │                                                                │
  │  DPO LOSS FUNCTION (simplified):                               │
  │                                                                │
  │  "Make the model MORE likely to produce user_kept"             │
  │  "Make the model LESS likely to produce ai_output"             │
  │  "By exactly the right amount (controlled by β parameter)"     │
  │                                                                │
  │  L_DPO = -log σ(β · (log π(chosen) - log π(rejected)))       │
  │                                                                │
  │  π = model's probability of generating that text               │
  │  β = temperature (how aggressively to shift preferences)       │
  │  σ = sigmoid (squashes to 0-1 range)                          │
  │                                                                │
  └────────────────────────────────────────────────────────────────┘
```

### 3.4 LoRA (Low-Rank Adaptation) — Efficient Fine-Tuning

```
  THE PROBLEM WITH FINE-TUNING:
  ═════════════════════════════

  Llama 8B model = 8 BILLION parameters = ~16 GB of weights

  Full fine-tuning: update ALL 8B parameters
  → Needs 80+ GB GPU RAM
  → Produces 16 GB model file per user
  → 1000 users = 16 TB of models 💀

  LoRA SOLUTION:
  ═══════════════

  Instead of updating all weights, add a TINY adapter:

  ┌────────────────────────────────────────────────────────┐
  │                                                        │
  │  Original weight matrix W (4096 × 4096 = 16M params)  │
  │                                                        │
  │  LoRA adds:  W' = W + (A × B)                         │
  │                                                        │
  │  Where A is (4096 × 8) and B is (8 × 4096)            │
  │  = only 65K params instead of 16M!                     │
  │                                                        │
  │  ┌──────────┐     ┌───┐   ┌──────────┐               │
  │  │          │     │   │   │          │               │
  │  │  4096    │     │ 8 │   │  4096    │               │
  │  │  ×       │  =  │ × │ × │  ×       │               │
  │  │  4096    │     │4096│   │  8       │               │
  │  │          │     │   │   │          │               │
  │  │ FROZEN W │     │ A │   │ B        │               │
  │  │(original)│     │   │   │          │               │
  │  └──────────┘     └───┘   └──────────┘               │
  │   16M params      65K params (trainable!)              │
  │                                                        │
  │  RESULT:                                               │
  │  • Base model: 16 GB (shared by ALL users)             │
  │  • Per-user adapter: ~2-10 MB                          │
  │  • 1000 users = 16 GB + 10 GB = 26 GB total ✅        │
  │  • Training: fits on a single GPU (8 GB VRAM)          │
  │                                                        │
  └────────────────────────────────────────────────────────┘

  FOR SAID (Tier 3 approach):

  ┌────────────────────────────────────────────────────────┐
  │                                                        │
  │  Base model: Llama 8B (runs on user's Mac with MLX)    │
  │  Per-user LoRA: trained on their 200+ corrections      │
  │  Swap LoRA at inference: load user's adapter on login   │
  │                                                        │
  │  User A speaks → load adapter_A.bin (5 MB)             │
  │  User B speaks → load adapter_B.bin (5 MB)             │
  │                                                        │
  └────────────────────────────────────────────────────────┘
```

### 3.5 Embeddings (Said Already Uses These)

```
  CONCEPT:
  ════════

  Convert text into a list of numbers (vector) that captures MEANING.

  "EMIAC mein kaam"  ──►  [0.23, -0.81, 0.45, 0.12, ..., 0.67]
                              256 numbers (dimensions)

  Similar meanings → similar vectors → small distance:

  "EMIAC mein kaam"    ←── distance: 0.15 ──→  "EMIAC ka project"
  "Weather today"      ←── distance: 0.92 ──→  "EMIAC mein kaam"
                                                (very different!)

  SAID USES EMBEDDINGS FOR:

  ┌──────────────────────────────────────────────────────────┐
  │                                                          │
  │  1. RAG (Retrieval-Augmented Generation):                │
  │     Embed the current transcript → find similar past     │
  │     corrections → inject as examples in the prompt       │
  │                                                          │
  │  2. Vocab Selection (BM25 + cosine similarity):          │
  │     Embed the transcript → find relevant vocab terms     │
  │     → only include those in the prompt (not all 200)     │
  │                                                          │
  │  Provider: Gemini text-embedding-004 (256 dimensions)    │
  │  Storage: SQLite (preference_vectors table)              │
  │                                                          │
  └──────────────────────────────────────────────────────────┘
```

### 3.6 ONNX (How to Ship a Model in an App)

```
  THE DEPLOYMENT PROBLEM:
  ═══════════════════════

  Training framework         Production app
  (Python, PyTorch)          (Rust, C++, mobile)
       │                          │
       │    Different worlds!     │
       │    PyTorch doesn't       │
       │    run in a Rust app     │
       └──────────┬───────────────┘
                  │
             ONNX SOLVES THIS

  ┌──────────────────────────────────────────────────────────┐
  │                                                          │
  │  ONNX = Open Neural Network Exchange                     │
  │                                                          │
  │  1. Train model in Python/PyTorch                        │
  │  2. Export to .onnx file (universal format)              │
  │  3. Load in ANY language via onnxruntime                 │
  │                                                          │
  │  Python ──export──► model.onnx ──load──► Rust            │
  │  (training)         (portable)           (production)    │
  │                                                          │
  │  onnxruntime-rs crate: runs ONNX models from Rust       │
  │  • CPU inference: ~1-5ms for small models                │
  │  • No Python dependency in the shipped app               │
  │  • Model file bundled like any other resource             │
  │                                                          │
  └──────────────────────────────────────────────────────────┘
```

---

## Part 4: The Three Tiers (What We Can Build)

```
  ┌───────────────────────────────────────────────────────────────────┐
  │                         TIER 1 (NOW)                              │
  │                    "Smart Prompt Engineering"                      │
  │                                                                   │
  │  What:  Aliases + vocab hints + RAG examples in the prompt        │
  │  Model: Cloud LLM (Groq Llama), NO per-user training             │
  │  Data:  STT aliases, corrections, edit_events (for retrieval)     │
  │  Score: ~85%                                                      │
  │  Cost:  Free (Groq) / cheap (gateway)                            │
  │  Ship:  ✅ Already shipped                                        │
  │                                                                   │
  │  CEILING: The cloud LLM doesn't know YOUR accent. It follows     │
  │  exact alias hints but can't generalize to novel distortions.     │
  ├───────────────────────────────────────────────────────────────────┤
  │                         TIER 2 (NEXT)                             │
  │              "Per-User Phonetic Correction Model"                 │
  │                                                                   │
  │  What:  Tiny character-level model that learns YOUR distortions   │
  │  Model: ~5M params, ONNX, runs on CPU in <2ms                    │
  │  Data:  stt_replacements table (transcript_form → correct_form)  │
  │  Score: ~95%+ (for known vocab terms)                             │
  │  Cost:  Zero at inference (runs locally)                          │
  │  Ship:  2-3 weeks to build                                       │
  │                                                                   │
  │  THIS IS THE SWEET SPOT. Read Part 5 below.                      │
  ├───────────────────────────────────────────────────────────────────┤
  │                         TIER 3 (FUTURE)                           │
  │              "Per-User LoRA Fine-Tuned LLM"                       │
  │                                                                   │
  │  What:  DPO fine-tuning on user's edit_events, LoRA adapters     │
  │  Model: Llama 8B base + per-user LoRA (~5 MB per user)           │
  │  Data:  200+ edit_events per user                                 │
  │  Score: ~98%+ (handles everything — style, vocab, tone)           │
  │  Cost:  GPU for training (~$2/user), local inference on Mac       │
  │  Ship:  2-3 months (needs training infra + MLX integration)       │
  │                                                                   │
  │  THE WISPRFLOW APPROACH. Maximum quality, maximum complexity.     │
  └───────────────────────────────────────────────────────────────────┘
```

---

## Part 5: Tier 2 Deep Dive — The Phonetic Correction Model

### What It Does

```
  BEFORE (current):
  ═════════════════

  Local speech: "meah mein bahut kaam hai"
                │
                ▼
  Alias table: meah not found ❌
                │
                ▼
  LLM prompt: "fix garbled words" → LLM ignores instruction ❌
                │
                ▼
  Output: "Meah mein bahut kaam hai"  ← WRONG


  AFTER (with Tier 2 model):
  ══════════════════════════

  Local speech: "meah mein bahut kaam hai"
                │
                ▼
  ┌─────────────────────────────────┐
  │   PHONETIC CORRECTION MODEL     │
  │   (tiny, local, <2ms)          │
  │                                 │
  │   For each token:               │
  │   "meah" → is this garbled?     │
  │         → YES (confidence 0.93) │
  │         → nearest vocab: EMIAC  │
  │         → similarity: 0.87     │
  │         → REPLACE ✅            │
  │                                 │
  │   "mein" → is this garbled?     │
  │         → NO (real Hindi word)  │
  │         → KEEP AS-IS ✅         │
  └─────────────────────────────────┘
                │
                ▼
  Cleaned: "EMIAC mein bahut kaam hai"
                │
                ▼
  LLM polish (easy job now — no garbled words to fix)
                │
                ▼
  Output: "EMIAC mein bahut kaam hai."  ← CORRECT ✅
```

### Architecture

```
  ┌──────────────────────────────────────────────────────────────┐
  │                   TIER 2 ARCHITECTURE                        │
  │                                                              │
  │                                                              │
  │   ┌──────────────────────────────────────────────────┐      │
  │   │              TRAINING PIPELINE                    │      │
  │   │           (runs periodically / on demand)         │      │
  │   │                                                   │      │
  │   │   SQLite DB                                       │      │
  │   │   ┌─────────────────────┐                         │      │
  │   │   │ stt_replacements    │                         │      │
  │   │   │ meac → EMIAC        │──┐                      │      │
  │   │   │ mnc → EMIAC         │  │                      │      │
  │   │   │ mecobs → Macobs     │  │  Extract             │      │
  │   │   │ aneten → n8n        │  │  training pairs      │      │
  │   │   │ mvac → EMIAC        │  │                      │      │
  │   │   └─────────────────────┘  │                      │      │
  │   │                            ▼                      │      │
  │   │   ┌─────────────────────────────────────┐        │      │
  │   │   │  TRAINING DATA GENERATOR             │        │      │
  │   │   │                                      │        │      │
  │   │   │  For each alias (meac → EMIAC):      │        │      │
  │   │   │  1. Generate character n-grams        │        │      │
  │   │   │  2. Compute phonetic features         │        │      │
  │   │   │  3. Generate synthetic distortions    │        │      │
  │   │   │     (meax, meec, meach, meak, ...)    │        │      │
  │   │   │  4. Label: garbled=1, vocab=EMIAC     │        │      │
  │   │   │                                      │        │      │
  │   │   │  For common words (main, mein, hai):  │        │      │
  │   │   │  Label: garbled=0 (keep as-is)        │        │      │
  │   │   └──────────────┬──────────────────────┘        │      │
  │   │                  │                                │      │
  │   │                  ▼                                │      │
  │   │   ┌─────────────────────────────────────┐        │      │
  │   │   │  MODEL TRAINING                      │        │      │
  │   │   │                                      │        │      │
  │   │   │  Architecture: Character-level        │        │      │
  │   │   │  encoder + classifier head            │        │      │
  │   │   │                                      │        │      │
  │   │   │  Input:  "meah" (char sequence)       │        │      │
  │   │   │  Output: (is_garbled: 0.93,           │        │      │
  │   │   │          vocab_id: 0 [=EMIAC],        │        │      │
  │   │   │          confidence: 0.87)            │        │      │
  │   │   │                                      │        │      │
  │   │   │  Loss: cross-entropy on garbled +     │        │      │
  │   │   │        cross-entropy on vocab_id      │        │      │
  │   │   │                                      │        │      │
  │   │   │  Framework: PyTorch → export ONNX     │        │      │
  │   │   └──────────────┬──────────────────────┘        │      │
  │   │                  │                                │      │
  │   │                  ▼                                │      │
  │   │        correction_model.onnx (~2 MB)             │      │
  │   │        vocab_index.json (term → id mapping)      │      │
  │   └──────────────────────────────────────────────────┘      │
  │                                                              │
  │                                                              │
  │   ┌──────────────────────────────────────────────────┐      │
  │   │              INFERENCE PIPELINE                   │      │
  │   │        (runs on every voice recording)            │      │
  │   │                                                   │      │
  │   │   Transcript: "meah aur mecobs mein kaam hai"     │      │
  │   │                    │                              │      │
  │   │                    ▼                              │      │
  │   │   ┌──────────────────────────────────────┐       │      │
  │   │   │  TOKENIZER                            │       │      │
  │   │   │  Split: [meah, aur, mecobs, mein,     │       │      │
  │   │   │          kaam, hai]                    │       │      │
  │   │   └──────────────┬───────────────────────┘       │      │
  │   │                  │                                │      │
  │   │                  ▼  (for each token)              │      │
  │   │   ┌──────────────────────────────────────┐       │      │
  │   │   │  ONNX MODEL (correction_model.onnx)  │       │      │
  │   │   │                                      │       │      │
  │   │   │  "meah"   → garbled=0.93, id=0 ✅    │       │      │
  │   │   │  "aur"    → garbled=0.02, keep  ✅   │       │      │
  │   │   │  "mecobs" → garbled=0.91, id=1 ✅    │       │      │
  │   │   │  "mein"   → garbled=0.01, keep  ✅   │       │      │
  │   │   │  "kaam"   → garbled=0.01, keep  ✅   │       │      │
  │   │   │  "hai"    → garbled=0.01, keep  ✅   │       │      │
  │   │   │                                      │       │      │
  │   │   │  Vocab index: 0=EMIAC, 1=Macobs      │       │      │
  │   │   └──────────────┬───────────────────────┘       │      │
  │   │                  │                                │      │
  │   │                  ▼                                │      │
  │   │   Corrected: "EMIAC aur Macobs mein kaam hai"    │      │
  │   │                    │                              │      │
  │   │                    ▼                              │      │
  │   │   [continue to LLM polish as normal]              │      │
  │   └──────────────────────────────────────────────────┘      │
  │                                                              │
  └──────────────────────────────────────────────────────────────┘
```

### The Model Architecture (Character-Level)

```
  WHY CHARACTER-LEVEL (not word-level)?
  ═════════════════════════════════════

  Word-level models need a vocabulary of all possible words.
  But STT garbles produce INFINITE variations:
    meac, meah, mef, mvac, meax, meak, meech, meaक, yarmiac...

  A word-level model would never have seen "meah" in training.
  A character-level model sees: m-e-a-h and reasons about each character.

  MODEL STRUCTURE:
  ════════════════

  Input: "meah" → [m, e, a, h] → [12, 4, 0, 7] (char indices)
                                        │
                                        ▼
                              ┌──────────────────┐
                              │ Character         │
                              │ Embedding Layer   │
                              │ (32 dims per char)│
                              └────────┬─────────┘
                                       │
                            [32] [32] [32] [32]   (4 vectors)
                                       │
                                       ▼
                              ┌──────────────────┐
                              │ 1D Convolutions   │
                              │ (capture local    │
                              │  char patterns)   │
                              │                   │
                              │ "me" pattern      │
                              │ "ea" pattern      │
                              │ "ah" pattern      │
                              │ "meah" pattern    │
                              └────────┬─────────┘
                                       │
                                       ▼
                              ┌──────────────────┐
                              │ Global Max Pool   │
                              │ (fixed-size repr  │
                              │  regardless of    │
                              │  input length)    │
                              └────────┬─────────┘
                                       │
                                 [128] vector
                                       │
                          ┌────────────┼────────────┐
                          │            │            │
                          ▼            ▼            ▼
                   ┌───────────┐ ┌──────────┐ ┌──────────┐
                   │ Is Garbled│ │ Vocab ID │ │Confidence│
                   │ Head      │ │ Head     │ │ Head     │
                   │           │ │          │ │          │
                   │ sigmoid   │ │ softmax  │ │ sigmoid  │
                   │ → 0.93    │ │ → id=0   │ │ → 0.87   │
                   │(yes,junk) │ │(=EMIAC)  │ │          │
                   └───────────┘ └──────────┘ └──────────┘

  TOTAL PARAMETERS: ~2-5 million (tiny!)
  INFERENCE TIME: <1ms per token on CPU
  MODEL FILE SIZE: ~2-8 MB as ONNX

  COMPARE:
  • GPT-4: 1,800,000 million params = 1.8 trillion
  • Llama 8B: 8,000 million params
  • Said correction model: 5 million params
  • That's 0.0003% of GPT-4!
```

### Training Data Augmentation

```
  THE COLD START PROBLEM:
  ═══════════════════════

  A new user has 0 corrections → can't train a model.
  After 10 recordings, maybe 3 corrections → not enough.
  We need ~50-100 examples per vocab term to train well.

  SOLUTION: Data Augmentation (generate synthetic training data)

  ┌────────────────────────────────────────────────────────────┐
  │                                                            │
  │  REAL DATA (from stt_replacements):                        │
  │  meac → EMIAC (seen 3 times)                               │
  │  mnc  → EMIAC (seen 1 time)                                │
  │                                                            │
  │  AUGMENTED DATA (synthetically generated):                 │
  │                                                            │
  │  Character substitution:                                   │
  │    meac → meec, maac, meuc, meag, meab, meak               │
  │                                                            │
  │  Character deletion:                                       │
  │    meac → mac, mec, mea, eac                               │
  │                                                            │
  │  Character insertion:                                      │
  │    meac → meaac, meeac, meacr, ameac                       │
  │                                                            │
  │  Character swap:                                           │
  │    meac → maec, meac, meca                                 │
  │                                                            │
  │  Phonetic variants (using phonetic_key logic):             │
  │    emiac → emiag, emiak, emeac, imiac, emyac               │
  │                                                            │
  │  NEGATIVE EXAMPLES (real words that should NOT match):     │
  │    main, mein, mac, men, mean, meal, meat, ...             │
  │    → label: garbled=0 (keep as-is)                         │
  │                                                            │
  │  2 real examples → 200+ training examples after aug!       │
  │                                                            │
  └────────────────────────────────────────────────────────────┘
```

### How It Fits Into Said's Pipeline

```
  CURRENT PIPELINE:
  ═════════════════

  Audio → Local speech → [PRE-LLM ALIAS SUB] → [LLM POLISH] → Paste
                       (exact match only)    (prompt hints)
                       handles ~60%          handles ~20% more
                                             misses ~20%

  WITH TIER 2 MODEL:
  ═══════════════════

  Audio → Local speech → [PRE-LLM ALIAS SUB] → [CORRECTION MODEL] → [LLM POLISH] → Paste
                      (exact match only)     (neural, <2ms)       (much easier
                      handles ~60%           handles ~30% more     job now!)
                                             novel distortions     handles ~8% more
                                                                   misses ~2%

  INSERTION POINT IN CODE (voice.rs):

  ┌────────────────────────────────────────────────────────────────┐
  │                                                                │
  │  // Step 1: Exact alias substitution (already built ✅)        │
  │  let result = stt_replacements::apply_exact_safe(              │
  │      &stt_transcript_raw, &stt_replacement_rules,              │
  │  );                                                            │
  │                                                                │
  │  // Step 2: Neural correction model (NEW — Tier 2)             │
  │  let corrected = correction_model::correct(                    │
  │      &result.text,                                             │
  │      &vocab_terms,    // user's vocab list                     │
  │      &model,          // loaded ONNX model                     │
  │  );                                                            │
  │                                                                │
  │  // Step 3: LLM polish (existing — now much easier job)        │
  │  // ... build prompt, call Groq, stream tokens ...             │
  │                                                                │
  └────────────────────────────────────────────────────────────────┘
```

### Retraining Loop

```
  ┌──────────────────────────────────────────────────────────────┐
  │                     CONTINUOUS LEARNING                       │
  │                                                              │
  │                                                              │
  │  Day 1: User installs Said                                   │
  │  ├── No corrections yet                                      │
  │  ├── Model: not trained (skip correction step)               │
  │  └── Pipeline: alias sub + LLM only                          │
  │                                                              │
  │  Day 3: User has 5 corrections                               │
  │  ├── Still too few for training                              │
  │  ├── But aliases are working for exact matches               │
  │  └── Pipeline: alias sub + LLM (improving!)                  │
  │                                                              │
  │  Day 7: User has 20 corrections across 3 vocab terms         │
  │  ├── 🔔 THRESHOLD MET: trigger first training                │
  │  ├── Augment to ~500 examples                                │
  │  ├── Train model (~30 seconds on CPU)                        │
  │  ├── Export ONNX → save to app data directory                │
  │  └── Pipeline: alias sub + MODEL + LLM (big jump!)          │
  │                                                              │
  │  Day 14: User has 50 corrections, 2 new vocab terms          │
  │  ├── Retrain with expanded data                              │
  │  ├── Model now knows more distortion patterns                │
  │  └── Accuracy: ~95%                                          │
  │                                                              │
  │  Day 30: User has 150 corrections                            │
  │  ├── Model is very accurate for their accent                 │
  │  ├── Most garbled words caught before LLM sees them          │
  │  └── Accuracy: ~97%+                                         │
  │                                                              │
  │  RETRAIN TRIGGERS:                                           │
  │  • Every 10 new corrections (batched)                        │
  │  • When a new vocab term is added                            │
  │  • On app startup if corrections > last_trained_count + 10   │
  │                                                              │
  └──────────────────────────────────────────────────────────────┘
```

---

## Part 6: Tier 3 Deep Dive — Per-User DPO Fine-Tuning

```
  FOR WHEN TIER 2 ISN'T ENOUGH:
  ═════════════════════════════

  Tier 2 fixes garbled WORDS.
  Tier 3 fixes garbled words + STYLE + TONE + FORMATTING.

  Example corrections Tier 3 learns that Tier 2 can't:

  ┌──────────────────────────────────────────────────────────┐
  │  TRANSCRIPT           LLM OUTPUT          USER WANTED    │
  ├──────────────────────────────────────────────────────────┤
  │  "please check        "Please check       "Check this    │
  │   this once"           this once."         once, please." │
  │                                            (user always   │
  │                                             moves please  │
  │                                             to the end)   │
  ├──────────────────────────────────────────────────────────┤
  │  "do hazaar           "₹2000 ka bill"     "2000 ka bill" │
  │   ka bill"                                 (user never    │
  │                                             uses ₹ sign)  │
  ├──────────────────────────────────────────────────────────┤
  │  "kal meeting         "Kal meeting         "Kal meeting  │
  │   hai saat baje"       hai 7 baje."        hai 7:00 PM." │
  │                                            (user always   │
  │                                             adds AM/PM)   │
  └──────────────────────────────────────────────────────────┘

  These are STYLE preferences, not vocab corrections.
  A character-level model can't learn these.
  Only a full LLM fine-tuned with DPO can.


  TIER 3 ARCHITECTURE:
  ════════════════════

  ┌──────────────────────────────────────────────────────────┐
  │                                                          │
  │  CLOUD TRAINING SERVICE                                  │
  │  (runs when user has 200+ corrections)                   │
  │                                                          │
  │  ┌────────────────────────────────────────────┐         │
  │  │  1. Export user's edit_events               │         │
  │  │     (transcript, ai_output, user_kept)      │         │
  │  │                                             │         │
  │  │  2. Format as DPO pairs:                    │         │
  │  │     chosen  = user_kept                     │         │
  │  │     rejected = ai_output                    │         │
  │  │     prompt  = system + transcript           │         │
  │  │                                             │         │
  │  │  3. Train LoRA adapter with DPO loss        │         │
  │  │     Base: Llama 8B                          │         │
  │  │     LoRA rank: 8-16                         │         │
  │  │     Training: ~15 min on A100               │         │
  │  │     Cost: ~$2 per training run              │         │
  │  │                                             │         │
  │  │  4. Export LoRA adapter (~5 MB)              │         │
  │  │     Ship back to user's device              │         │
  │  └──────────────────┬─────────────────────────┘         │
  │                     │                                    │
  │                     ▼                                    │
  │  ┌────────────────────────────────────────────┐         │
  │  │  LOCAL INFERENCE (on user's Mac)            │         │
  │  │                                             │         │
  │  │  Base model: Llama 8B via MLX               │         │
  │  │  (Apple Silicon optimized, runs on M1+)     │         │
  │  │                                             │         │
  │  │  + User's LoRA adapter: adapter_abhishek    │         │
  │  │                                             │         │
  │  │  Inference: ~50ms for polish                │         │
  │  │  (vs ~800ms cloud Groq currently)           │         │
  │  │                                             │         │
  │  │  BONUS: completely offline! no API costs!   │         │
  │  └────────────────────────────────────────────┘         │
  │                                                          │
  └──────────────────────────────────────────────────────────┘
```

---

## Part 7: Implementation Roadmap

```
  ┌─────────────────────────────────────────────────────────────────┐
  │                        PHASE 1 (NOW — Week 1-2)                 │
  │                     "Maximize Tier 1"                           │
  │                                                                 │
  │  ✅ Pre-LLM exact alias substitution (DONE — this session)     │
  │  ✅ Alias cap enforcement at 15 per term (DONE — this session) │
  │  □  Re-enable Local speech biasing for high-weight terms            │
  │  □  Auto-store new distortions on each user correction          │
  │  □  Try Llama 3.3 70B (may follow phonetic instructions)        │
  │  □  Run pipeline quality suite, target 90%                      │
  │                                                                 │
  │  EFFORT: ~3-5 days                                              │
  │  EXPECTED: 85% → 90%                                           │
  ├─────────────────────────────────────────────────────────────────┤
  │                      PHASE 2 (Week 3-5)                         │
  │              "Build Tier 2 Correction Model"                    │
  │                                                                 │
  │  □  Python training script:                                     │
  │     • Extract training data from SQLite                         │
  │     • Data augmentation (char-level perturbations)              │
  │     • Train character CNN + classifier                          │
  │     • Export to ONNX                                            │
  │                                                                 │
  │  □  Rust inference integration:                                 │
  │     • Add onnxruntime-rs dependency                             │
  │     • Load model on backend startup                             │
  │     • Insert correction step in voice.rs pipeline               │
  │                                                                 │
  │  □  Retrain trigger:                                            │
  │     • Background task checks correction count                   │
  │     • Spawns Python training when threshold met                 │
  │     • Hot-reloads new ONNX model                                │
  │                                                                 │
  │  EFFORT: ~2-3 weeks                                             │
  │  EXPECTED: 90% → 95%+                                          │
  ├─────────────────────────────────────────────────────────────────┤
  │                      PHASE 3 (Month 2-3)                        │
  │                "Build Tier 3 DPO Pipeline"                      │
  │                                                                 │
  │  □  Cloud training service (Modal / RunPod):                    │
  │     • DPO training script with LoRA                             │
  │     • Data export from user's SQLite                            │
  │     • Adapter upload/download                                   │
  │                                                                 │
  │  □  Local inference with MLX:                                   │
  │     • Bundle Llama 8B (quantized, ~4 GB)                        │
  │     • LoRA adapter hot-swap                                     │
  │     • Replace Groq API calls with local inference               │
  │                                                                 │
  │  □  Privacy & opt-in:                                           │
  │     • User consent for cloud training                           │
  │     • Differential privacy on training data                     │
  │     • Option to stay cloud-only (Tier 1+2)                      │
  │                                                                 │
  │  EFFORT: ~6-8 weeks                                             │
  │  EXPECTED: 95% → 98%+                                          │
  │  BONUS: offline mode, zero API costs, faster inference          │
  └─────────────────────────────────────────────────────────────────┘
```

---

## Part 8: Key Decision — Why Not Jump to Tier 3?

```
  "Why not just do DPO fine-tuning now?"

  ┌────────────────────────────────┬───────────────────────────────┐
  │          TIER 2                │          TIER 3               │
  ├────────────────────────────────┼───────────────────────────────┤
  │ Solves: garbled proper nouns   │ Solves: everything            │
  │ (80% of current failures)     │ (vocab + style + formatting)  │
  ├────────────────────────────────┼───────────────────────────────┤
  │ Training: 30 sec on CPU       │ Training: 15 min on A100 GPU  │
  │ Cost: $0                      │ Cost: ~$2 per user per train  │
  ├────────────────────────────────┼───────────────────────────────┤
  │ Cold start: 20 corrections    │ Cold start: 200+ corrections  │
  │ (works after 1 week)          │ (works after 1-2 months)      │
  ├────────────────────────────────┼───────────────────────────────┤
  │ Model size: 2-8 MB            │ Model size: 4 GB (quantized)  │
  │ (bundled in app)              │ (needs download)              │
  ├────────────────────────────────┼───────────────────────────────┤
  │ Inference: <2ms on CPU        │ Inference: ~50ms on M1+ GPU   │
  │ (any machine)                 │ (Apple Silicon only initially) │
  ├────────────────────────────────┼───────────────────────────────┤
  │ Privacy: 100% local           │ Cloud training (data leaves)  │
  │                               │ Local inference (data stays)  │
  ├────────────────────────────────┼───────────────────────────────┤
  │ Complexity: medium             │ Complexity: high              │
  │ (Python script + ONNX)        │ (cloud infra + MLX + LoRA)   │
  ├────────────────────────────────┼───────────────────────────────┤
  │ Ship in: 2-3 weeks            │ Ship in: 2-3 months           │
  └────────────────────────────────┴───────────────────────────────┘

  ANSWER: Tier 2 gives you 80% of the benefit at 20% of the cost.
  Build Tier 2 first, then layer Tier 3 on top when you have users
  with enough correction data to make DPO training worthwhile.

  They're ADDITIVE, not exclusive:
  Tier 1 (aliases) catches exact matches
  Tier 2 (neural) catches novel distortions of known vocab
  Tier 3 (DPO LLM) catches style/tone/formatting preferences
```

---

## Glossary

```
  TERM                 PLAIN ENGLISH
  ════                 ═════════════
  SFT                  Supervised Fine-Tuning — train on (input, correct_output) pairs
  DPO                  Direct Preference Optimization — train on (chosen > rejected) pairs
  RLHF                 RL from Human Feedback — train a reward model, then RL against it
  LoRA                 Low-Rank Adaptation — tiny trainable adapter on top of frozen model
  ONNX                 Model file format that works in any language (Python → Rust)
  MLX                  Apple's ML framework optimized for Apple Silicon (M1/M2/M3)
  Embedding            Text → numbers vector that captures meaning
  RAG                  Retrieval-Augmented Generation — find similar examples, add to prompt
  BM25                 Text search algorithm (like Google, but for local DB)
  Phonetic Key         Simplified sound representation ("meac" → "MK", "EMIAC" → "EMK")
  Character CNN        Neural network that reads text character-by-character
  Cold Start           The period before enough data exists to train a model
  Data Augmentation    Generate synthetic training data from real examples
  Inference            Running a trained model to get predictions (vs training it)
  Quantization         Compress model weights (float32 → int4) to reduce size/speed
  Adapter              Small add-on to a base model (LoRA is the most common type)
  Cross-Entropy        Loss function — "how wrong was the prediction?" (lower = better)
  Sigmoid              Squash any number into 0-1 range (used for probabilities)
  Softmax              Like sigmoid but for multiple classes (probabilities sum to 1)
```
