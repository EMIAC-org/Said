# AirNote Meeting AI Benchmark

## ami-es2002a-5min

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 4721 ms; verifier latency: 2991 ms
Counts: `{"draft_actions": 0, "draft_decisions": 0, "verified_actions": 0, "verified_decisions": 0, "filtered_actions": 0, "filtered_decisions": 0}`

### Summary
The meeting served as a kickoff for a project to design a new remote control that is original, trendy, and user-friendly. The project manager, Laura, outlined a three-stage design process involving individual work followed by meetings. Attendees introduced themselves: Laura as project manager, David as an industrial designer, Andrew as a marketing expert, and Greg (or Craig) as a digital interface specialist. The team then participated in an icebreaker activity where they drew their favorite animals on a whiteboard and described their characteristics. Andrew drew a beagle and valued physical affection, personality, and good health, while another participant drew a monkey described as small, cute, and furry.

### Actions
- None explicit.

### Decisions
- None explicit.

### Judge
```json
{
  "summary_accuracy": 8,
  "summary_coverage": 8,
  "action_precision": 10,
  "decision_precision": 10,
  "chat_grounding": 9,
  "overall": 9,
  "issues": [
    "Summary mentions 'Greg (or Craig)' but transcript shows Speaker 4 says 'I'm Greg' and Laura later says 'Craig', which is a transcription ambiguity, not a MoM error.",
    "Chat answer for 'PM-ready brief' correctly marks uncertainty and does not invent facts.",
    "No action items or decisions were invented; MoM and chat correctly reflect absence of explicit commitments.",
    "Chat answer for 'What important unresolved questions or risks remain?' is accurate given transcript lacks such discussion."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

The main topic of the meeting was the kickoff for a project to design a new remote control that is original, trendy, and user-friendly. Laura, the project manager, outlined a three-stage design process involving individual work followed by meetings.

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the transcript, no concrete decisions were explicitly made.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

The transcript does not contain any explicit action items or follow-ups.

Q4. What important unresolved questions or risks remain?

Based on the transcript, no unresolved questions or risks are explicitly discussed. The meeting focuses on introductions, the project brief, the design process, and an icebreaker activity. There is no mention of outstanding issues, concerns, or risks.

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

Based on the provided transcript, here is a PM-ready brief. The transcript does not contain enough information to fully populate all requested sections, so uncertainties are marked.

**Problem**
The team is tasked with designing a new remote control that is "original, trendy, and user-friendly" [01:49].

**Approach**
The project manager, Laura, outlined a three-stage design process. Each stage consists of individual work followed by a meeting [02:25]. The meeting served as the project kickoff [00:56].

**Decisions**
No decisions were captured in the transcript.

**Risks**
No risks were discussed in the transcript.

**Next Steps**
The immediate next step captured is an icebreaker activity where team members draw their favorite animal on a whiteboard [02:34]. The transcript does not specify the next formal project step after this activity concludes.

**Uncertainty**
- The specific deliverables for each of the three design stages are not mentioned.
- The timeline for the project beyond the 25-minute kickoff meeting is not defined.
- The roles of the team members (David, Andrew, Greg/Craig) beyond their titles were not elaborated upon.

## ami-es2002a-20min

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 8168 ms; verifier latency: 7171 ms
Counts: `{"draft_actions": 3, "draft_decisions": 0, "verified_actions": 3, "verified_decisions": 0, "filtered_actions": 3, "filtered_decisions": 0}`

### Summary
Laura (project manager) kicked off the project to design a trendy, user-friendly remote control with a €25 selling price and a €12.50 production cost cap, targeting €15 million in international sales. The team—David (industrial designer), Andrew (marketing), and Craig (digital interface)—introduced themselves and completed an icebreaker activity. Discussion explored international design implications (regional characters, market positioning), the potential for a premium, multi-functional device (combining TV, DVD, satellite, and possibly home lighting), and user pain points like losing the remote. The meeting concluded with Laura assigning individual work for the next functional design stage, with a follow-up meeting scheduled in 30 minutes. No final decisions were made; the team discussed possibilities such as extra functionality and a unique selling point, but these remained tentative.

### Actions
- Work on the actual work and design of the remote control | assignee: David | due: before next meeting in 30 minutes | evidence: as the industrial designer you're going to be working on you know the actual work and design of it
- Work on user interface and technical functions | assignee: Craig | due: before next meeting in 30 minutes | evidence: for our user interface technical functions i guess that's you know like what we've been talking about what it'll actually do
- Think about requirements the remote control has to fulfill | assignee: Andrew | due: before next meeting in 30 minutes | evidence: and uh more to being executive you'll be just thinking about what it actually what you know what requirements it has to has to fulfill

### Decisions
- None explicit.

### Judge
```json
{
  "summary_accuracy": 8,
  "summary_coverage": 8,
  "action_precision": 7,
  "decision_precision": 10,
  "chat_grounding": 9,
  "overall": 8,
  "issues": [
    "Action item for Andrew is misassigned: transcript says 'more to being executive' (likely marketing), but MoM assigns 'Think about requirements' to Andrew, which is correct, but the evidence quote is garbled and does not clearly support the assignment.",
    "Action item for Craig is slightly vague: transcript says 'for our user interface technical functions i guess that's you know like what we've been talking about what it'll actually do', which is not a crisp assignment, but MoM treats it as a firm action item.",
    "Chat answer for unresolved questions correctly notes no decision on scope, but the transcript shows Laura explicitly said 'that's up to us', which is a decision to defer, not a pure unresolved question—minor nuance."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

The main topic of the meeting was the kick-off for a project to design a new, trendy, and user-friendly remote control.

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the transcript, no concrete decisions were explicitly made. The team discussed possibilities such as extra functionality and a unique selling point, but these remained tentative.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

Based on the transcript, the explicit action items and follow-ups assigned at the end of the meeting are:

- **David** (industrial designer): Work on the actual work and design of the remote control.
- **Craig** (digital interface): Work on user interface and technical functions.
- **Andrew** (marketing): Think about the requirements the remote control has to fulfill.

All are due before the next meeting in 30 minutes.

Q4. What important unresolved questions or risks remain?

Based on the transcript, the following unresolved questions or risks remain:

- **Scope of functionality**: It is unresolved whether the remote control will be limited to television features or include extra functions (e.g., controlling lighting, combining multiple devices). Andrew explicitly asked if they are keeping a “design commitment to television features” or going beyond, and Laura said “that’s up to us” [17:32–18:02].
- **Production cost vs. feature trade-off**: David noted that the €12.50 production cost cap is a main factor limiting how much can be crammed into the device [18:04–18:16]. The risk of exceeding the cost target while adding premium or multi-device features remains unaddressed.
- **International design implications**: The team discussed regional differences (characters, keypads, market positioning) but did not resolve how these will be handled in the design [09:11–10:15].
- **Unique selling point**: Laura mentioned the need for “some kind of unique selling point” but no decision was made on what that would be [17:57–18:02].
- **User pain points**: Issues like losing the remote were raised, but no solution (e.g., a beeping locator) was committed to [15:03–15:20].

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

Based on the transcript, here is a PM-ready brief. Note that the meeting was an initial kick-off, so many elements are tentative and no formal decisions were made.

**Problem**
The team needs to design a new remote control that is "trendy and user-friendly." The project has specific financial targets: a €25 selling price and a production cost cap of €12.50, with a goal of €15 million in international sales.

**Approach**
The design process is structured into three stages, each involving individual work followed by a team meeting. The immediate next stage is the "functional design stage." The team discussed several possibilities for the product's direction, including:
*   Designing for international markets, considering regional characters and different market positioning based on price.
*   Creating a premium, multi-functional device that combines controls for TV, DVD, satellite, and potentially other systems like home lighting.
*   Addressing user pain points, such as the ease of losing the remote (e.g., by including a "find" feature) and making the device more streamlined (e.g., a touch screen).

**Decisions**
No final decisions were made during this meeting. The discussion of extra functionality and a unique selling point remained tentative. The project manager, Laura, stated that the direction is "up to us" but noted the production cost cap is a main limiting factor.

**Risks & Uncertainties**
*   **Unclear Market Position:** It is uncertain whether the €25 price point positions the remote as a premium product or a standard one. The team lacks competitive pricing data to benchmark this.
*   **Undefined Scope:** The team has not committed to whether the remote's features will be limited to television or will expand to control other devices. This ambiguity could lead to scope creep or a product that fails to meet the cost target.
*   **Production Cost Constraint:** The €12.50 production cost cap is a significant constraint that may limit the inclusion of advanced features like a touch screen or multi-device control.

**Next Steps**
Individual work is assigned to be completed before the next meeting in 30 minutes:
*   **David (Industrial Designer):** Work on the actual physical design of the remote control.
*   **Craig (Digital Interface):** Work on the user interface and technical functions.
*   **Andrew (Marketing):** Think about the requirements the remote control has to fulfill.

## ami-en2001a-30min-mix

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 9798 ms; verifier latency: 5564 ms
Counts: `{"draft_actions": 4, "draft_decisions": 0, "verified_actions": 2, "verified_decisions": 0, "filtered_actions": 2, "filtered_decisions": 0}`

### Summary
The team discussed Steve's feedback, which emphasized avoiding duplication of effort and prioritizing a working prototype by week six. They debated the data structure for the browser, considering whether to store importance measures at the word or utterance level, ultimately leaning towards utterance-level storage for simplicity and better skimming. The conversation covered the integration of offline calculations with the NightXML framework, the handling of multi-level meeting series data, and the separation of search functionality using SQL. Key unresolved questions include the feasibility of loading all data into memory and the exact method for linking their importance data to NightXML's existing utterance IDs.

### Actions
- Try to build a class that concatenates different waveforms in memory for audio skimming | assignee: Speaker 3 | due: None | evidence: what I think I might try and build is basically a class that you just feed it a linked list of different waveforms, it'll just string them all together
- Ask Jonathan about lazy loading and memory issues with NightXML | assignee: Speaker 1 | due: None | evidence: I'll probably ask Jonathan about it.

### Decisions
- None explicit.

### Judge
```json
{
  "summary_accuracy": 8,
  "summary_coverage": 8,
  "action_precision": 10,
  "decision_precision": 10,
  "chat_grounding": 9,
  "overall": 9,
  "issues": [
    "Summary mentions 'debated the data structure... ultimately leaning towards utterance-level storage' which is accurate, but the summary does not mention the alternative 'virtual meeting' fallback plan discussed at length.",
    "Chat answer for 'What important unresolved questions or risks remain?' invents the risk 'Audio skimming feasibility' as an unresolved question. The transcript shows Speaker 3 committing to build the class and Speaker 1 saying 'That's okay, that we can do', indicating it was accepted as feasible, not an open risk.",
    "Chat answer for 'PM-ready brief' lists 'Utterance‑level importance storage is the working decision (not yet formally ratified)' under Decisions. The transcript shows a leaning but no explicit decision was made; this is a slight overstatement."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

The main topic was settling on the data structure for the browser, specifically whether to store importance measures at the word or utterance level, and how to integrate offline calculations with the NightXML framework. The team also discussed prioritizing a working prototype by week six per Steve's feedback.

Q2. What concrete decisions were explicitly made? If none, say none.

None.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

Based on the transcript, the explicit action items or follow-ups are:

- **Speaker 3** will try to build a class that concatenates different waveforms in memory for audio skimming.
  *Evidence*: "what I think I might try and build is basically a class that you just feed it a linked list of different waveforms, it'll just string them all together" [17:00 Speaker 3]

- **Speaker 1** will ask Jonathan about lazy loading and memory issues with NightXML.
  *Evidence*: "I'll probably ask Jonathan about it." [20:09 Speaker 1]

Q4. What important unresolved questions or risks remain?

Based on the transcript, the key unresolved questions and risks are:

- **Feasibility of loading all data into memory**: There is uncertainty about whether the system can load the entire meeting series data into memory without performance issues. Speaker 1 notes, “I'm still confused because I thought that's just what Jonathan said, that we can't do, like load a massive document of that size” [18:03], and later says, “I'll probably ask Jonathan about it” [20:09] regarding lazy loading and memory constraints.

- **Linking importance data to NightXML utterance IDs**: The team is unsure how to tie their importance measures to NightXML’s existing data structure. Speaker 2 mentions, “But there's no ID for an utterance, I think. It's just for individual words” [27:02], and Speaker 4 adds, “I have only seen that the individual words have got an ID” [28:00]. This leaves open the question of how to reliably reference utterances.

- **Handling multi-level meeting series data**: The group debates whether to treat the whole meeting series as one massive document or to create a virtual meeting structure with separate levels. Speaker 1 suggests, “if we can't actually load the whole thing in… create a virtual meeting out of the whole meeting series” [21:12–21:25], but no final decision is reached.

- **Audio skimming feasibility**: While Speaker 3 proposes building a class to concatenate waveforms in memory [16:56–17:09], it remains untested whether the audio player can accept such input and handle word-level cuts smoothly.

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**PM‑Ready Brief**

**Problem**
The team needs to settle the data structure for the browser before building the week‑six prototype. The core tension is whether to store importance measures at the word level or the utterance level, and how to integrate offline calculations with the NightXML framework without exceeding memory limits.

**Approach**
- **Utterance‑level storage** is the emerging consensus. It simplifies audio skimming, reduces memory, and naturally produces sentence‑like summaries. Word‑level calculations can still be done internally and then averaged or thresholded to produce utterance scores.
- **Offline calculations** will pre‑compute importance for every utterance in the corpus. The online browser will only load utterance IDs, scores, speaker labels, and timestamps – not raw audio or full annotations.
- **NightXML integration** will be done by creating a lightweight XML file that ties utterance scores to NightXML’s existing utterance IDs (or timestamps if IDs are missing). The browser will rely on NightXML’s event model for synchronisation.
- **Search** will be handled separately via SQL, returning timestamps that are fed back into NightXML to trigger display updates.
- **Multi‑level meeting series** may be handled by either loading all utterances into memory (if feasible) or by creating a “virtual meeting” XML that treats meetings as segments and segments as utterances, reusing the same display logic.

**Decisions**
- **Utterance‑level importance storage** is the working decision (not yet formally ratified).
- **Audio skimming** will be done by concatenating waveforms in memory (Speaker 3 to prototype a class for this).
- **Search stays outside NightXML**, using SQL, and only passes timestamps back to the framework.

**Risks & Uncertainties**
- **Memory feasibility** – It is unclear whether loading all utterances for a 70‑hour meeting series will fit in RAM. Jonathan previously warned against loading everything; Speaker 1 will ask him about lazy‑loading options.
- **Utterance IDs** – It is uncertain whether NightXML’s utterance segments already have IDs. If not, the team will need to add them or rely on timestamps.
- **Word‑level filtering** – Averaging word scores may dilute importance; a threshold or pre‑filter may be needed, but the impact on audio skimming is not yet resolved.
- **Multi‑level abstraction** – The “virtual meeting” approach is a fallback; the team has not decided whether to pursue it or attempt a single in‑memory load.

**Next Steps**
1. **Speaker 3** – Prototype an in‑memory waveform concatenation class for audio skimming.
2. **Speaker 1** – Ask Jonathan about lazy loading and memory constraints in NightXML.
3. **Team** – Inspect NightXML code to confirm whether utterance segments have IDs and how timestamps are structured.
4. **Team** – Finalise the data structure decision (utterance‑level vs. word‑level) after the NightXML review.
5. **All** – Align offline output formats with the agreed data structure so the week‑six prototype can integrate pre‑annotated data.
