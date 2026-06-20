# AirNote Meeting AI Benchmark

## ami-es2002a-5min

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 2902 ms; verifier latency: 2029 ms
Counts: `{"draft_actions": 0, "draft_decisions": 0, "verified_actions": 0, "verified_decisions": 0, "filtered_actions": 0, "filtered_decisions": 0}`

### Summary
The meeting began with informal technical setup and introductions. Laura, the project manager, outlined the project to design a new remote control that is original, trendy, and user-friendly. The design process involves three stages of individual work followed by meetings. The team then participated in an icebreaker activity where they drew their favorite animals and described their characteristics on a whiteboard.

### Actions
- None explicit.

### Decisions
- None explicit.

### Judge
```json
{
  "summary_accuracy": 9,
  "summary_coverage": 9,
  "action_precision": 10,
  "decision_precision": 10,
  "chat_grounding": 9,
  "overall": 9,
  "issues": [
    "Chat answer for 'What important unresolved questions or risks remain?' states 'no unresolved questions or risks mentioned', which is accurate based on the transcript, but the PM brief correctly identifies implicit uncertainties (e.g., unspecified deliverables, unscoped requirements). This is a minor inconsistency in strictness, not a factual error.",
    "MoM summary is accurate and covers the key points, but it omits the specific team member names and roles mentioned during introductions (Laura, David, Andrew, Greg/Craig). This is a minor coverage gap."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

Based on the transcript, the main topic of the meeting was the kickoff for a project to design a new remote control that is original, trendy, and user-friendly. The project manager, Laura, outlined the design process, which involves three stages of individual work followed by meetings.

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the provided transcript, no concrete decisions were explicitly made.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

The transcript does not contain any explicit action items or follow-ups with assigned owners.

Q4. What important unresolved questions or risks remain?

Based on the transcript, there are no unresolved questions or risks mentioned. The discussion covers introductions, the project brief (designing an original, trendy, user-friendly remote control), the three-stage design process, and an icebreaker activity, but no uncertainties, blockers, or risks are raised.

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**Project Brief: New Remote Control Design**

**Problem**
The team is tasked with designing a new remote control that is original, trendy, and user-friendly (per Laura, the project manager, at [01:49]).

**Approach**
The design process will follow three iterative stages, each consisting of individual work followed by a team meeting ([02:25]). The kickoff meeting included introductions and an icebreaker activity where team members drew their favorite animals and described their characteristics on a whiteboard ([02:41]).

**Decisions**
No explicit decisions are captured in the transcript.

**Risks**
No risks are explicitly stated in the transcript.

**Next Steps**
- **Unassigned:** Begin the first stage of individual design work (implied by the three-stage process outlined at [02:25]).
- **Unassigned:** Schedule the first follow-up meeting after individual work (implied by the process at [02:25]).

**Uncertainty**
- The transcript does not specify what deliverables or criteria are expected for each stage.
- The brief’s requirements (“original, trendy, and user-friendly”) are stated but not yet detailed or scoped.

## ami-es2002a-20min

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 8523 ms; verifier latency: 7583 ms
Counts: `{"draft_actions": 3, "draft_decisions": 0, "verified_actions": 3, "verified_decisions": 0, "filtered_actions": 3, "filtered_decisions": 0}`

### Summary
Laura (project manager) kicked off the project to design a new trendy, user-friendly remote control with a target selling price of 25 euros and a production cost cap of 12.50 euros, aiming for international sales. The team, including David (industrial designer), Andrew (marketing expert), and Craig (digital interface), introduced themselves and participated in an icebreaker activity. The group discussed financial constraints, international market considerations like regional character sets and varying price appeal, and potential features such as combining multiple device controls, adding a 'find my remote' beep function, and exploring a more streamlined or touch-screen design. The meeting concluded with Laura assigning initial individual work for the next functional design stage, scheduled to reconvene in 30 minutes.

### Actions
- Work on the actual industrial design of the remote control | assignee: None | due: in 30 minutes | evidence: as the industrial designer you're going to be working on you know the actual work and design of it
- Work on user interface and technical functions | assignee: None | due: in 30 minutes | evidence: for our user interface technical functions i guess that's you know like what we've been talking about what it'll actually do
- Think about marketing requirements the product has to fulfill | assignee: None | due: in 30 minutes | evidence: and uh more to being executive you'll be just thinking about what it actually what you know what requirements it has to has to fulfill

### Decisions
- None explicit.

### Judge
```json
{
  "summary_accuracy": 8,
  "summary_coverage": 8,
  "action_precision": 3,
  "decision_precision": 10,
  "chat_grounding": 7,
  "overall": 6,
  "issues": [
    "MoM action items assign owners (David, Craig, Andrew) that are not explicitly stated in the transcript. The transcript only mentions roles, not names, for these tasks.",
    "MoM action item 'Think about marketing requirements...' invents the assignee 'Andrew' and the task framing 'marketing requirements' is a slight extrapolation from 'what requirements it has to fulfill'.",
    "Chat answer for action items correctly assigns owners based on roles, but the MoM fails to do so, creating inconsistency.",
    "Chat answer for 'unresolved questions' includes 'How the international market will affect the design' as unresolved, but the transcript shows some discussion and partial resolution (e.g., character sets, price appeal).",
    "Chat answer for 'PM-ready brief' states 'No explicit decisions are captured' which is accurate, but the MoM also correctly lists no decisions.",
    "MoM summary mentions 'adding a find my remote beep function' as a discussed feature, which is accurate, but the chat answer for 'main topic' omits this specific feature."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

The main topic of the meeting was the kick-off for a project to design a new, trendy, user-friendly remote control. The team discussed financial constraints (a 25-euro selling price and a 12.50-euro production cost cap), international market considerations, and potential features. The meeting concluded with the project manager assigning initial individual work for the next functional design stage.

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the provided transcript and meeting intelligence, no concrete decisions were explicitly made. The meeting intelligence also confirms that the decisions list is empty.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

Based on the transcript, the explicit action items and follow-ups assigned for the next 30 minutes are:

- **Industrial design work:** The industrial designer (David) will work on the actual design of the remote control. (Owner: David, ~16:28–16:33)
- **User interface and technical functions:** The digital interface role (Craig) will work on what the remote control will actually do. (Owner: Craig, ~16:37–16:46)
- **Marketing requirements:** The marketing expert (Andrew) will think about the requirements the product has to fulfill. (Owner: Andrew, ~16:46–16:56)

Q4. What important unresolved questions or risks remain?

Based on the transcript, the following unresolved questions and risks are explicitly mentioned:

**Unresolved Questions:**
- Whether the remote control should include features that go beyond television (e.g., controlling lighting, notes about what to watch) or stay focused on TV features. This was raised by Andrew at [17:27–17:47] and not resolved.
- How the international market will affect the design, including regional character sets and varying price appeal across countries [09:11–10:15].
- What the 25-euro selling price represents in terms of product positioning (e.g., premium vs. standard) and how it compares to other remote controls on the market [10:55–12:11].

**Risks:**
- The production cost cap of 12.50 euros may limit how many features can be included, as noted by David at [18:04–18:16].
- The need to balance a "trendy" and "user-friendly" design with the financial constraints and potential complexity of international requirements [08:51–09:44].

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**Project Brief – Remote Control Design (Kick‑off)**
*Prepared for the Project Manager (Laura)*

---

### Problem
The team must design a **trendy, user‑friendly remote control** that can be sold internationally at a **retail/wholesale price of €25**, with a **production cost cap of €12.50** (50% of selling price). The product must appeal across different markets, support varied character sets, and differentiate itself from standard remotes that users rarely evaluate consciously.

### Approach
- **Three‑stage iterative design** with individual work followed by team meetings (first functional design stage in 30 minutes).
- **Roles assigned**:
  - David – industrial design (form, materials, physical design)
  - Craig – user interface & technical functions (what the remote actually does)
  - Andrew – marketing requirements (market positioning, feature demands)
- **Early feature exploration** included:
  - Combining controls for multiple devices (TV, DVD, satellite, etc.)
  - “Find my remote” beep function
  - Streamlined or touch‑screen form factor
  - Extra functionality (e.g., home lighting, viewing notes)
- **Constraints acknowledged**: production cost cap limits how many features can be included.

### Decisions
No explicit decisions are captured in the transcript.

### Risks & Uncertainties
- **Uncertainty – Selling price definition**: It is unclear whether the €25 target is a wholesale or retail price, which affects margin calculations and feature scope.
- **Uncertainty – Market price sensitivity**: The €25 price point may appeal in some regions (e.g., London) but not others (e.g., Greece); no market data is available to validate this.
- **Constraint – Production cost cap**: The €12.50 ceiling may limit the ability to add advanced features (e.g., touch screen, multi‑device control, beeper).
- **Uncertainty – Feature scope**: The team has not yet committed to whether the remote will remain TV‑focused or expand to control other devices/systems.

### Next Steps (Functional Design Stage – due in 30 minutes)
| Action | Owner | Status |
|--------|-------|--------|
| Work on the actual industrial design of the remote | David | Assigned |
| Work on user interface and technical functions | Craig | Assigned |
| Think about marketing requirements the product must fulfill | Andrew | Assigned |
| Reconvene for functional design review | All | Scheduled (~10 to 12) |

*Note: The transcript does not specify who will send detailed instructions; Laura mentioned instructions would be emailed.*

## ami-en2001a-30min-mix

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 9878 ms; verifier latency: 4546 ms
Counts: `{"draft_actions": 3, "draft_decisions": 1, "verified_actions": 1, "verified_decisions": 0, "filtered_actions": 1, "filtered_decisions": 0}`

### Summary
The team discussed Steve's feedback about duplication of effort and the need for a prototype by week six. They debated the data structure for the meeting browser, focusing on whether to store importance measures at the word or utterance level, ultimately leaning toward utterance-level storage for simplicity and memory efficiency. The conversation covered how to handle multi-level meeting series, with ideas including creating a virtual meeting XML document or using a single large document with zoom levels. They also discussed the integration with the NightXML framework, the separation of online and offline processing, and the need to investigate the audio player's capabilities and NightXML's lazy loading behavior.

### Actions
- Look at NightXML to see how far what we want is compatible with what NightXML offers | assignee: None | due: None | evidence: we then actually need to, well, go back first of all and look at night XML to see how far that which we want is compatible with that which night XML offers us.

### Decisions
- None explicit.

### Judge
```json
{
  "summary_accuracy": 7,
  "summary_coverage": 8,
  "action_precision": 5,
  "decision_precision": 10,
  "chat_grounding": 8,
  "overall": 7,
  "issues": [
    "MoM summary is accurate but misses the explicit discussion about storing importance at utterance vs. word level and the leaning toward utterance-level.",
    "MoM action item 'Look at NightXML' has no assignee, which is correct per transcript, but the MoM JSON includes an 'assignee' field set to null; this is acceptable but the action item is vague.",
    "Chat answer for 'concrete decisions' correctly states none were made, but the MoM 'decisions' array is empty, which is accurate.",
    "Chat answer for 'action items' correctly identifies only one action item with no owner, matching the transcript.",
    "Chat answer for 'unresolved questions' invents 'NightXML compatibility and lazy loading' as a risk with specific timestamps, but the transcript does not explicitly state this as an unresolved question; it is inferred.",
    "Chat answer for 'PM-ready brief' invents 'Prototype integration' as a next step, which is not explicitly stated in the transcript.",
    "Chat answer for 'PM-ready brief' states 'no explicit decisions are captured', which is accurate, but the brief itself invents 'Approach' details not explicitly decided (e.g., 'Store importance measures at the utterance level').",
    "MoM summary invents 'they debated the data structure' but does not capture the specific leaning toward utterance-level, which is a minor omission."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

The main topic was settling on the data structure for the meeting browser, specifically whether to store importance measures at the word or utterance level, and how to handle multi-level meeting series within the NightXML framework.

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the provided transcript, no explicit decisions were captured.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

Based on the transcript, the only explicit action item is:

- **Look at NightXML** to see how far what we want is compatible with what NightXML offers (no owner assigned).

Q4. What important unresolved questions or risks remain?

Based on the transcript, the following unresolved questions and risks remain:

- **NightXML compatibility and lazy loading**: The team needs to investigate how far their desired data structure is compatible with what NightXML offers, and whether NightXML’s lazy loading can handle the full meeting series without memory issues (e.g., the known error when loading all data). [02:01 Speaker 1, 18:46 Speaker 1, 19:02 Speaker 1]
- **Audio player capabilities**: It is unknown whether the audio player can accept input from memory, handle concatenated waveforms, or support word-level seeking, which affects decisions on utterance-level vs. word-level storage and skimming. [15:52 Speaker 1, 16:00 Speaker 3, 17:30 Speaker 1]
- **Utterance IDs in NightXML**: It is unclear whether utterances already have IDs in the NightXML data; if not, the team will need to add them or find another way to tie importance weights to utterances. [27:00 Speaker 2, 27:50 Speaker 1]
- **Memory feasibility for full-series loading**: There is uncertainty about whether loading all utterances and rankings for a 70-hour meeting series into memory is feasible, or whether a multi-level virtual-meeting approach is needed. [18:03 Speaker 1, 22:05 Speaker 1, 23:02 Speaker 1]
- **Handling unimportant words in audio skimming**: If low-importance words are filtered out, it is unresolved how the audio player would skip them without storing cut-out segments, and whether buffering/concatenation in memory is practical. [15:34 Speaker 1, 16:42 Speaker 1]

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**Brief**

**Problem**
The team needs to settle on a data structure and integration approach for the meeting browser prototype (due week six) while addressing Steve’s feedback about duplication of effort. Key open questions include how to store importance measures (word vs. utterance level), how to handle multi-level meeting series, and how to align with the NightXML framework without overloading memory.

**Approach**
- Store importance measures at the **utterance level** rather than word level, to reduce memory usage and simplify audio/text display. Algorithms may still compute at word level internally, then average or aggregate to utterances.
- For multi-level meeting series, consider either:
  - Loading one large document with zoom levels (displaying different granularities by adjusting start/end and zoom), or
  - Creating a “virtual meeting” XML document that treats the series as a meeting, meetings as segments, and segments as utterances, reusing the same display logic.
- Keep online processing minimal; most computation (information density, summaries) happens offline. The browser acts primarily as a viewer on precomputed XML data.
- Use NightXML’s event model for synchronization across display elements; search will query a separate SQL database and then push a timestamp into NightXML to trigger updates.
- Investigate NightXML’s lazy loading and the audio player’s capabilities to determine feasibility of loading full series data vs. chunked metadata.

**Decisions**
No explicit decisions are captured in the transcript. The team is leaning toward utterance-level storage and exploring NightXML compatibility, but no final decision is recorded.

**Risks**
- **Memory/performance uncertainty**: It is unclear whether loading all utterances for a 70‑hour meeting series into NightXML will work; the team recalls that loading everything previously caused errors. Lazy loading may mitigate this, but its behavior is not yet understood.
- **Audio player limitations**: The team does not know whether the audio player can accept in‑memory streams or concatenate waveforms on the fly; this could affect skimming implementation.
- **NightXML compatibility**: The extent to which the desired data model fits NightXML’s structure is unknown; investigation is needed before committing to the framework.

**Next Steps**
- **Investigate NightXML** – review its code and documentation to assess compatibility with the proposed data model (unassigned).
- **Check audio player capabilities** – determine if it supports in‑memory audio concatenation and timestamp‑based seeking (unassigned).
- **Clarify lazy loading behavior** – ask Jonathan or examine NightXML to understand what is loaded by default and what can be loaded on demand (unassigned).
- **Prototype integration** – once data structure and framework questions are resolved, build a working prototype using pre‑annotated data and baseline components (implied next phase, unassigned).
