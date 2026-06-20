# AirNote Meeting AI Benchmark

## ami-es2002a-5min

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 4081 ms; verifier latency: 3119 ms
Counts: `{"draft_actions": 0, "draft_decisions": 0, "verified_actions": 0, "verified_decisions": 0, "filtered_actions": 0, "filtered_decisions": 0}`

### Summary
The meeting began with informal technical setup and introductions. Laura, the project manager, outlined the project to design a new remote control that is original, trendy, and user-friendly. She described a three-stage design process involving individual work followed by meetings. The team then participated in an icebreaker activity where they drew their favorite animals on a whiteboard and described their characteristics. Andrew drew a beagle and valued physical affection and personality, while another participant drew a monkey and described it as small, cute, and furry.

### Actions
- None explicit.

### Decisions
- None explicit.

### Judge
```json
{
  "summary_accuracy": 9,
  "summary_coverage": 8,
  "action_precision": 10,
  "decision_precision": 10,
  "chat_grounding": 9,
  "overall": 9,
  "issues": [
    "Summary mentions 'another participant drew a monkey' but the transcript attributes this to Speaker 3 (Craig), not an unnamed participant. Minor inaccuracy.",
    "Chat PM brief states 'Team members received only a basic project announcement' but the transcript shows Andrew saying 'Yeah, that's enough' which could imply satisfaction, not necessarily lack of detail. Slight over-interpretation.",
    "Chat PM brief lists 'Unclear scope/constraints' as a risk, but the transcript does not explicitly discuss this as a risk; it is an inferred risk, not grounded in a direct statement."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

Based on the transcript, the main topic of the meeting was the kickoff for a project to design a new remote control. Laura, the project manager, stated the remote control is supposed to be "original, trendy, and user-friendly" [01:49–01:58] and outlined a three-stage design process [02:25–02:28].

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the provided transcript and meeting intelligence, no concrete decisions were explicitly made. The meeting intelligence also confirms that the decisions list is empty.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

The transcript does not contain any explicit action items or follow-ups.

Q4. What important unresolved questions or risks remain?

The transcript does not contain any unresolved questions or risks. The meeting covered introductions, the project brief, the design process, and an icebreaker activity, but no risks or open issues were discussed.

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**PM‑Ready Brief**

**Problem**
The team needs to design a new remote control that is original, trendy, and user‑friendly (Laura, [01:49–01:58]). The project is in its kickoff phase, and the team has just been introduced to the brief and to each other.

**Approach**
Laura outlined a three‑stage design process: each stage consists of individual work followed by a team meeting, repeated three times ([02:25–02:28]). The kickoff meeting included introductions and an icebreaker activity to build team familiarity.

**Decisions**
No explicit decisions are captured in the transcript or the provided decisions list.

**Risks**
- **Uncertainty about prior information:** Team members received only a basic project announcement; it is unclear whether everyone has the same level of detail about the design stages or expectations (David, [02:12–02:19]; Laura, [02:06–02:10]).
- **Unclear scope/constraints:** The brief is high‑level (“original, trendy, user‑friendly”) with no further specifications, which may lead to misalignment later.

**Next Steps**
- Begin the first stage of individual design work (implied by the three‑stage process, [02:25–02:28]).
- Reconvene for the first design‑stage meeting (timing not specified in the transcript).

*Note: The transcript does not contain explicit commitments, deadlines, or assigned owners for the next steps.*

## ami-es2002a-20min

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 8576 ms; verifier latency: 6105 ms
Counts: `{"draft_actions": 3, "draft_decisions": 0, "verified_actions": 3, "verified_decisions": 0, "filtered_actions": 3, "filtered_decisions": 0}`

### Summary
Laura kicked off the project to design a new remote control, introducing herself as project manager and having David (industrial designer), Andrew (marketing), and Greg (digital interface) introduce themselves. The team did an icebreaker drawing favorite animals and discussing their characteristics. They then discussed project finances, noting the remote will sell for 25 euros with a production cost cap of 12.50 euros, and considered international market implications. The group brainstormed features like combining multiple device controls, adding new functionality (e.g., lighting, notes), a 'find me' beep, and a more streamlined chic design, while acknowledging production cost constraints. Laura wrapped up by outlining the next functional design stage and individual work assignments before the next meeting in 30 minutes.

### Actions
- Work on the actual design of the remote control | assignee: David | due: None | evidence: as the industrial designer you're going to be working on you know the actual work and design of it
- Work on user interface and technical functions | assignee: Greg | due: None | evidence: for our user interface technical functions i guess that's you know like what we've been talking about what it'll actually do
- Think about requirements the remote control has to fulfill | assignee: Andrew | due: None | evidence: and and uh more to being executive you'll be just thinking about what it actually what you know what requirements it has to has to fulfill

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
    "MoM summary mentions 'Greg (digital interface)' but transcript says 'I'm Greg and I'm a digital interface' which is ambiguous; could be a role title rather than a name, but not a major error.",
    "Action item for Andrew says 'more to being executive' which is a transcription error; the actual phrase is unclear, but the assignment is loosely supported.",
    "Chat answer for unresolved questions correctly identifies uncertainties, but the 'risk' about production cost cap is presented as a risk rather than an explicit decision, which is accurate.",
    "No invented decisions or owners detected; decisions list is correctly empty.",
    "Chat answers are well-grounded in the transcript with specific timestamps and quotes."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

The main topic was the kick‑off meeting for designing a new remote control, including introductions, an icebreaker, discussion of project finances and international considerations, and brainstorming features and design ideas.

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the provided transcript and meeting intelligence, no concrete decisions were explicitly made. The meeting intelligence also confirms that the decisions list is empty.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

Based on the transcript, the explicit action items and follow-ups with owners are:

- **David** will work on the actual design of the remote control. (Laura: “as the industrial designer you're going to be working on you know the actual work and design of it”)
- **Greg** will work on user interface and technical functions. (Laura: “for our user interface technical functions i guess that's you know like what we've been talking about what it'll actually do”)
- **Andrew** will think about requirements the remote control has to fulfill. (Laura: “and and uh more to being executive you'll be just thinking about what it actually what you know what requirements it has to has to fulfill”)

Q4. What important unresolved questions or risks remain?

Based on the transcript, the following unresolved questions and risks are present:

- **Unresolved Question – Selling Price Context:** Andrew raised whether the 25-euro selling price is wholesale or retail, and Laura was unsure (00:08:28 – 00:08:51). This remains unresolved.
- **Unresolved Question – International Design Impact:** The team discussed whether international markets will affect the design (e.g., different characters, keypad styles, regional pricing appeal) but did not reach a conclusion (00:09:11 – 00:10:15).
- **Unresolved Question – Feature Scope:** Andrew asked whether the remote control should include features beyond television (e.g., lighting, notes) or stay committed to TV features. Laura said it’s up to the team but did not finalize the scope (00:17:27 – 00:18:02).
- **Risk – Production Cost Cap:** David noted that the production cost cap of 12.50 euros is a main factor limiting how many features can be included, implying a risk of feature creep versus cost constraints (00:18:04 – 00:18:16).

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**Project Brief – New Remote Control Design (Kick‑off)**

**Problem**
The team has been tasked with designing a new remote control that is “trendy and user‑friendly,” to be sold internationally at a retail price of €25, with a production cost cap of €12.50 (50% of selling price). The product must appeal across markets and potentially address common user frustrations (e.g., losing the remote, juggling multiple remotes, clunky design).

**Approach**
- Three‑stage design process with individual work followed by team meetings.
- First stage: functional design – each member works on their area (industrial design, user interface/technical functions, marketing requirements).
- Brainstormed features include: combining controls for multiple devices, adding new functionality (lighting, notes), a “find me” beep, and a more streamlined/chic design (e.g., touch screen).
- Production cost cap will constrain feature scope.

**Decisions**
No explicit decisions are captured in the transcript.

**Risks & Uncertainties**
- **Uncertainty:** Whether the remote should be limited to TV features or expand beyond television (e.g., home lighting, notes) – explicitly raised but not resolved.
- **Uncertainty:** How the €25 price point positions the product in different international markets (e.g., premium vs. standard).
- **Risk:** Production cost cap (€12.50) may limit the number of features that can be included.
- **Uncertainty:** Lack of benchmark data on what €25 buys in the remote‑control market.

**Next Steps**
- **David (Industrial Designer):** Work on the actual design of the remote control.
- **Greg (Digital Interface):** Work on user interface and technical functions.
- **Andrew (Marketing):** Think about requirements the remote control must fulfill.
- **All:** Reconvene in 30 minutes for the functional design stage meeting.

## ami-en2001a-30min-mix

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 9561 ms; verifier latency: 6010 ms
Counts: `{"draft_actions": 3, "draft_decisions": 0, "verified_actions": 1, "verified_decisions": 0, "filtered_actions": 1, "filtered_decisions": 0}`

### Summary
The team discussed Steve's feedback, which emphasized creating a prototype by week six and avoiding duplication of effort. They debated the data structure for the meeting browser, considering whether to store importance measures at the word or utterance level, ultimately leaning towards utterance-level storage for simplicity and better skimming. The group explored how to handle the display for a series of meetings, considering options like loading all data into memory versus creating a virtual meeting structure to manage scale. They also discussed the integration with the NightXML framework, the separation of online and offline processing, and the need to investigate the framework's capabilities, particularly regarding lazy loading and audio playback.

### Actions
- Ask Jonathan about the lazy loading behavior and memory limitations of the NightXML kit | assignee: Speaker 1 | due: None | evidence: I'll probably ask Jonathan about it.

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
    "MoM summary is accurate and covers key discussion points, but omits the explicit leaning towards utterance-level storage and the specific discussion about creating a 'virtual meeting' structure.",
    "MoM correctly identifies no explicit decisions were made, matching the transcript.",
    "Action item is correctly extracted with the stated owner (Speaker 1).",
    "Chat answers are well-grounded in the transcript, accurately reflecting the discussion, unresolved questions, and the lack of formal decisions.",
    "Chat answer for 'PM-ready brief' correctly marks uncertainty and does not invent decisions, though it synthesizes the 'approach' from discussion rather than explicit agreement."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

The main topic was settling on the data structure for the meeting browser, particularly whether to store importance measures at the word or utterance level, and how to handle display and memory for a series of meetings within the NightXML framework.

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the provided transcript and the meeting intelligence, no explicit decisions were captured. The meeting intelligence's "decisions" list is empty, and the transcript does not contain any formal decision-making statements.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

Based on the transcript, here are the explicit action items and follow-ups:

*   **Ask Jonathan about the lazy loading behavior and memory limitations of the NightXML kit.** (Owner: Speaker 1)

Q4. What important unresolved questions or risks remain?

Based on the transcript, the important unresolved questions or risks are:

- **Memory and lazy loading limitations of the NightXML framework**: The team is uncertain whether loading all utterance data for a series of meetings into memory is feasible, as Jonathan previously indicated it might not work. Speaker 1 plans to ask Jonathan about the lazy loading behavior and memory limitations (around [20:04]–[20:13]).
- **How to handle display for a meeting series**: It is unresolved whether to load all data into memory or create a virtual meeting structure to manage scale, and whether the NightXML framework can support the desired approach (around [20:04]–[22:05]).
- **Integration with NightXML and audio playback**: The team needs to investigate the framework’s capabilities, particularly regarding lazy loading, audio playback from memory, and how to synchronize data across different levels (around [01:54]–[02:34], [15:50]–[16:04]).
- **Utterance-level vs. word-level importance storage**: While leaning toward utterance-level storage, the team has not fully resolved how to handle unimportant words within utterances or how to filter them for audio skimming (around [14:51]–[15:48]).
- **Availability of utterance IDs in the data**: There is uncertainty about whether utterances have unique IDs in the existing data, which could affect how importance measures are tied to utterances (around [27:00]–[28:04]).

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**Brief**

**Problem**
The team needs to settle on a data structure and integration approach for the meeting browser to build a working prototype by week six, while avoiding duplication of effort and managing scale across a series of meetings.

**Approach**
- Store importance measures at the **utterance level** (not word level) to simplify audio skimming, display, and memory usage.
- Use the **NightXML framework** for online display and synchronization, leveraging its event model to reduce custom work.
- Keep offline processing separate; the browser will act primarily as a viewer of precomputed data.
- For the meeting series view, either load all utterance data into memory (if feasible) or create a **virtual meeting structure** that treats the series like a single meeting, with meetings as segments and segments as utterances.
- Search will leave the NightXML framework and query a SQL database, then feed timestamps back to NightXML to update all displays.

**Decisions**
No explicit decisions are captured in the transcript.

**Risks**
- **Memory/scale uncertainty**: It is unclear whether loading all utterance data for a full meeting series into memory is feasible. The NightXML kit already fails when loading all data for a project.
- **Lazy loading behavior unknown**: The team does not yet understand how NightXML’s lazy loading works or whether it can mitigate memory issues.
- **Utterance-level granularity may lose nuance**: Averaging word-level importance over utterances could dilute important words or include filler words, though a cutoff threshold is proposed.
- **Audio concatenation feasibility**: Building an in-memory audio stream from selected utterances is assumed to be straightforward but has not been verified.

**Next Steps**
- **Speaker 1** to ask Jonathan about NightXML’s lazy loading behavior and memory limitations.
- Investigate NightXML’s capabilities in detail, particularly around data representation, lazy loading, and audio playback.
- Decide whether to store importance at the utterance level or word level (leaning toward utterance level, but not finalized).
- Determine how to handle the meeting series display (load all vs. virtual meeting structure) based on NightXML investigation.
- Build a prototype by week six using pre-annotated data and baseline components where custom modules are not ready.
