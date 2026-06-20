# AirNote Meeting AI Benchmark

## ami-es2002a-5min

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 5461 ms; verifier latency: 3243 ms
Counts: `{"draft_actions": 0, "draft_decisions": 0, "verified_actions": 0, "verified_decisions": 0, "filtered_actions": 0, "filtered_decisions": 0}`

### Summary
The meeting began with informal technical setup and introductions. Laura, the project manager, outlined the project to design a new remote control that is original, trendy, and user-friendly. The design process involves three stages of individual work followed by meetings. The team then participated in an icebreaker activity where they drew their favorite animals on a whiteboard and described their characteristics. Andrew drew a beagle and valued physical affection, personality, and good health, while Greg drew a monkey that was small, cute, and furry.

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
  "chat_grounding": 10,
  "overall": 9,
  "issues": [
    "Summary mentions 'Greg drew a monkey' but transcript shows Speaker 3 (Greg) drew a monkey; the name is correct based on introductions, but the transcript does not explicitly confirm Speaker 3 is Greg during the drawing. However, the introduction sequence strongly implies it, so this is a minor note.",
    "Chat answer for 'What important unresolved questions or risks remain?' states 'no unresolved questions or risks mentioned' which is accurate per transcript, but the PM-ready brief correctly identifies uncertainties (e.g., unclear email contents, vague scope). This is a minor inconsistency in the chat's direct answer vs. the brief, but both are grounded."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

The main topic was the kickoff for a project to design a new remote control that is original, trendy, and user-friendly (Speaker 2, Laura, [01:32]–[01:58]).

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the provided transcript and the meeting intelligence, no concrete decisions were explicitly made.

The meeting intelligence confirms this, listing the decisions as empty. The transcript itself covers introductions, the project brief, the design process stages, and an icebreaker activity, but no formal decisions are recorded.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

The transcript does not contain any explicit action items or follow-ups.

Q4. What important unresolved questions or risks remain?

Based on the transcript, there are no unresolved questions or risks mentioned. The meeting focused on introductions, the project brief, and an icebreaker activity, without discussing any uncertainties or potential issues.

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**Project Brief: New Remote Control Design – Kickoff Meeting**

**Problem / Objective**
The team is tasked with designing a new remote control that is **original, trendy, and user-friendly** (per the project brief stated by Laura at [01:49]).

**Approach**
The design process will follow **three iterative stages**, each consisting of individual work followed by a team meeting ([02:25]). The team includes Laura (Project Manager), David (Industrial Designer), Andrew (Marketing Expert), and Greg (Digital Interface) ([01:12]–[01:27]).

**Decisions**
No explicit decisions are captured in the meeting intelligence or transcript.

**Risks & Unresolved Questions**
- **Uncertainty:** The transcript does not clarify what specific materials or instructions the team received via email beyond the basic project announcement ([02:10]–[02:19]). Laura was unsure what the team had received ([02:06]).
- **Uncertainty:** The detailed scope, constraints, or success criteria for “original, trendy, and user-friendly” were not discussed.

**Next Steps**
- Begin the first stage of individual design work (implied by the three-stage process outlined at [02:25]).
- The immediate next activity in the meeting was an icebreaker (drawing favorite animals), which was completed. No further meeting actions were assigned.

## ami-es2002a-20min

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 9863 ms; verifier latency: 6933 ms
Counts: `{"draft_actions": 3, "draft_decisions": 0, "verified_actions": 3, "verified_decisions": 0, "filtered_actions": 3, "filtered_decisions": 0}`

### Summary
Laura kicked off the project to design a new remote control, introducing the team (David, Andrew, and Craig) and outlining the three-stage design process. The group participated in an icebreaker by drawing favorite animals and discussing their characteristics. They then reviewed project finances, noting a target selling price of 25 euros and a production cost cap of 12.50 euros, and discussed the implications of international sales. The conversation shifted to brainstorming features, including combining multiple device controls, adding new functionalities like home lighting, and addressing common user frustrations such as losing the remote. The meeting concluded with Laura assigning initial tasks for the next stage and a brief discussion on whether the design should extend beyond television features, with production cost identified as a main constraint. No final decisions were made.

### Actions
- Work on the actual industrial design of the remote control | assignee: David | due: None | evidence: as the industrial designer you're going to be working on you know the actual work and design of it
- Work on user interface and technical functions | assignee: Craig | due: None | evidence: for our user interface technical functions i guess that's you know like what we've been talking about what it'll actually do
- Think about marketing requirements the product has to fulfill | assignee: Andrew | due: None | evidence: and uh more to being executive you'll be just thinking about what it actually what you know what requirements it has to has to fulfill

### Decisions
- None explicit.

### Judge
```json
{
  "summary_accuracy": 8,
  "summary_coverage": 8,
  "action_precision": 9,
  "decision_precision": 10,
  "chat_grounding": 9,
  "overall": 9,
  "issues": [
    "MoM summary mentions 'Craig' but transcript introduces 'Greg' as digital interface; name mismatch.",
    "MoM action item for Andrew says 'more to being executive' which is a transcription error but faithfully reproduced; no penalty.",
    "Chat answer for 'What concrete decisions were explicitly made?' correctly states none, matching transcript.",
    "Chat answer for action items correctly identifies owners David, Craig, and Andrew with accurate evidence.",
    "Chat PM brief correctly notes no decisions made and identifies risks from transcript.",
    "No invented facts, owners, or decisions detected."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

The main topic of the meeting was the kick-off for a project to design a new remote control. Laura, the project manager, introduced the team, outlined the three-stage design process, reviewed project finances (including a target selling price of 25 euros and a production cost cap of 12.50 euros), and led a brainstorming session on features and user frustrations. The meeting concluded with task assignments for the next stage.

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the provided transcript and meeting intelligence, no concrete decisions were explicitly made. The meeting intelligence confirms this with an empty decisions list.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

Based on the transcript, the explicit action items and follow-ups with owners are:

- **David**: Work on the actual industrial design of the remote control (mentioned at [16:28]–[16:33]).
- **Craig**: Work on user interface and technical functions (mentioned at [16:37]–[16:46]).
- **Andrew**: Think about marketing requirements the product has to fulfill (mentioned at [16:46]–[16:56]).

Q4. What important unresolved questions or risks remain?

Based on the transcript, the following unresolved questions and risks were identified:

- **Unresolved Question – Scope of Features**: Andrew explicitly asked whether the remote control design should include features that go beyond television functions, or if the team should keep a “design commitment to television features.” Laura responded that it was “up to us” and that a unique selling point might be needed, but no final direction was agreed upon [17:27–18:02].
- **Risk – Production Cost as a Constraint**: David identified the production cost cap (€12.50) as the main factor limiting how much functionality can be included, implying a risk that desired features may not fit within the budget [18:04–18:16].
- **Unresolved Question – International Design Implications**: The team discussed potential international design challenges (e.g., different character sets, regional pricing perceptions) but did not resolve how these would be addressed in the design [09:11–10:15].

No other explicit risks or unresolved questions are captured in the transcript.

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**Project Brief – New Remote Control Design (Kick‑off)**

**Problem**
The team has been asked to design a “trendy and user‑friendly” remote control that can be sold internationally for €25, with a production cost cap of €12.50 (50% of selling price). No existing market data or competitive benchmarks have been provided, and the team has not yet defined the product’s core feature set or target audience.

**Approach**
- Three‑stage design process with individual work followed by team meetings.
- Initial brainstorming covered: combining controls for multiple devices, adding non‑TV functions (e.g., home lighting, viewing notes), addressing common frustrations (losing the remote, bulky design), and exploring premium materials or touch‑screen interfaces.
- The team agreed that international markets may require different keypad layouts, character sets, and price‑positioning considerations.

**Decisions**
No explicit decisions were captured in this meeting.

**Risks & Uncertainties**
- **Unclear scope:** It is uncertain whether the remote should remain TV‑focused or expand into broader home‑control functions.
- **Cost vs. functionality:** The €12.50 production cap may limit how many extra features can be included (raised by David at [18:04–18:16]).
- **Market ambiguity:** The team lacks data on what €25 buys in different regions and whether the product will be positioned as a premium or standard offering.
- **User‑need assumptions:** Pain points (losing the remote, multiple remotes) are based on personal anecdotes, not validated research.

**Next Steps**
| Owner  | Task | Due |
|--------|------|-----|
| David  | Work on the actual industrial design of the remote control | Not specified |
| Craig  | Work on user interface and technical functions | Not specified |
| Andrew | Think about marketing requirements the product must fulfill | Not specified |

*Note: The next meeting is scheduled in 30 minutes (approx. 11:50), where the functional design stage will begin.*

## ami-en2001a-30min-mix

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 10164 ms; verifier latency: 7320 ms
Counts: `{"draft_actions": 3, "draft_decisions": 1, "verified_actions": 3, "verified_decisions": 0, "filtered_actions": 3, "filtered_decisions": 0}`

### Summary
The team discussed Steve's feedback about duplication of effort and the need for a prototype by week six. They debated the data structure for the meeting browser, focusing on whether to store importance measures at the word or utterance level. The conversation covered how to handle multi-level summaries (meeting series vs. individual meetings), the role of the NightXML framework for synchronization, and the separation of online and offline processing. They also considered using a virtual meeting structure to handle large corpora and the need to investigate NightXML's capabilities further. No final decision was reached on the word vs. utterance level; the team leaned towards utterance-level but agreed to investigate the audio player's capabilities first.

### Actions
- Look at NightXML to see how far what we want is compatible with what NightXML offers | assignee: None | due: None | evidence: we then actually need to, well, go back first of all and look at night XML to see how far that which we want is compatible with that which night XML offers us
- Check out how the audio player works | assignee: None | due: None | evidence: Maybe we can do that together at some point today that we checked out how the player works.
- Ask Jonathan about lazy loading and memory issues | assignee: Speaker 1 | due: None | evidence: I'll probably ask Jonathan about it.

### Decisions
- None explicit.

### Judge
```json
{
  "summary_accuracy": 7,
  "summary_coverage": 8,
  "action_precision": 6,
  "decision_precision": 10,
  "chat_grounding": 8,
  "overall": 7,
  "issues": [
    "MoM summary says 'No final decision was reached on the word vs. utterance level; the team leaned towards utterance-level but agreed to investigate the audio player's capabilities first.' This is accurate and grounded.",
    "MoM action item 'Look at NightXML...' has no assignee, which matches the transcript (no owner specified).",
    "MoM action item 'Check out how the audio player works' has no assignee, matching the transcript.",
    "MoM action item 'Ask Jonathan about lazy loading and memory issues' assigns Speaker 1, which matches the transcript.",
    "Chat answer for 'What concrete decisions were explicitly made?' correctly states no explicit decisions were captured.",
    "Chat answer for action items correctly lists the three items with owners only where specified.",
    "Chat PM brief correctly marks decisions as none and notes uncertainties.",
    "No invented facts, owners, or decisions detected in MoM or chat answers.",
    "Minor issue: MoM summary could have mentioned the 'virtual meeting' abstraction as a discussed approach, but omission is not a factual error.",
    "Chat PM brief mentions 'Speaker 1 will do this' for asking Jonathan, which is grounded."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

The main topic of the meeting was settling on the data structure for the meeting browser, specifically whether to store importance measures at the word level or the utterance level. The team also discussed how to handle multi-level summaries (meeting series vs. individual meetings), the role of the NightXML framework for synchronization, and the separation of online and offline processing.

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the provided decisions list, no explicit decisions were captured.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

Based on the transcript, the explicit action items are:

- Look at NightXML to see how far what we want is compatible with what NightXML offers (no owner specified) [02:01 Speaker 1]
- Check out how the audio player works (no owner specified) [11:50 Speaker 1]
- Ask Jonathan about lazy loading and memory issues (owner: Speaker 1) [20:04 Speaker 1]

Q4. What important unresolved questions or risks remain?

Based on the transcript, the following unresolved questions and risks remain:

- **Word-level vs. utterance-level storage**: The team leaned toward utterance-level importance but did not finalize the decision. They agreed to investigate the audio player’s capabilities first (Speaker 1, ~11:43–13:10; Speaker 3, ~11:35–11:54).
- **NightXML compatibility**: It is unclear how far the desired data structure aligns with what NightXML offers. The team needs to examine NightXML in detail (Speaker 1, ~01:54–02:22).
- **Memory and lazy loading**: There is uncertainty about whether loading the entire meeting series into memory is feasible. Speaker 1 plans to ask Jonathan about lazy loading and memory issues (Speaker 1, ~18:03–20:09).
- **Audio player capabilities**: The team does not yet know whether the audio player can handle word-level or utterance-level skimming, or accept audio from memory (Speaker 1, ~11:50–12:02; Speaker 3, ~16:00–16:32).
- **Utterance IDs**: It is unclear whether utterances have IDs in the existing data; if not, a workaround will be needed (Speaker 2, ~27:00–27:02; Speaker 4, ~27:04–27:18).

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**Problem**
The team needs to settle on a data structure for the meeting browser—specifically whether to store importance measures at the word level or the utterance level—and to understand how the NightXML framework and audio player constrain that choice. Steve’s feedback also pressures them to deliver a working prototype by week six (next week), which forces prioritization of the interface and reuse of pre‑annotated data.

**Approach**
- Build the prototype by focusing on the interface first, using pre‑annotated data and simple baselines where annotations are missing, then replace components later.
- Investigate NightXML’s event‑synchronization model to see how much of the desired multi‑level display (meeting series → meeting → segment) it can support out of the box.
- Explore the audio player’s capabilities (e.g., playing concatenated utterances from memory) to inform the word‑vs‑utterance decision.
- Consider a “virtual meeting” abstraction: treat a meeting series as a meeting, individual meetings as segments, and segments as utterances, so the same display code can work at multiple zoom levels.
- For offline processing, calculate importance at the word level but aggregate to the utterance level for storage and display; use a cutoff or pre‑filtering to suppress low‑importance words.
- For search, leave the NightXML framework and query a SQL database, then feed the resulting timestamp back into NightXML’s central manager to synchronize all display elements.

**Decisions**
No explicit decisions are captured in the transcript. The team leaned toward utterance‑level storage but agreed to investigate the audio player first.

**Risks / Uncertainties**
- *Uncertain* whether NightXML can handle the full meeting‑series data in memory; the existing project already fails with an out‑of‑memory error when loading everything. Lazy loading may help, but its exact behavior is unclear.
- *Uncertain* whether the audio player can play arbitrary word‑ or utterance‑level excerpts smoothly; the team has not yet tested it.
- *Uncertain* whether utterances have stable IDs in the current XML; if not, they may need to add them, which could conflict with NightXML’s ID conventions.
- *Risk* that the week‑six prototype deadline is tight given unresolved technical questions and one team member’s upcoming absence.

**Next Steps**
1. **Look at NightXML** to assess compatibility with the desired data model and multi‑level display (no assignee specified).
2. **Check how the audio player works**, ideally together today, to understand what granularity of playback is feasible (no assignee specified).
3. **Ask Jonathan about lazy loading and memory issues** – Speaker 1 will do this.
