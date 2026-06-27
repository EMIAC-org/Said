# AirNote Meeting AI Benchmark

## ami-es2002a-5min

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 4253 ms; verifier latency: 3292 ms
Counts: `{"draft_actions": 0, "draft_decisions": 0, "verified_actions": 0, "verified_decisions": 0, "filtered_actions": 0, "filtered_decisions": 0}`

### Summary
The meeting began with informal chatter and technical setup before Laura, the project manager, initiated the kickoff for a new remote control design project. Attendees introduced themselves: Laura as project manager, David as industrial designer, Andrew as marketing expert, and Greg as digital interface. Laura outlined the project brief to design an original, trendy, and user-friendly remote control, and described a three-stage iterative process of individual work followed by meetings. The group then engaged in an icebreaker activity where they drew their favorite animals on a whiteboard and described their characteristics.

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
    "Summary mentions 'Greg as digital interface' but transcript says 'I'm Greg, and I'm a digital interface' [01:27], which is a minor phrasing difference, not a factual error.",
    "Chat answer for 'PM-ready brief' lists a risk about 'Uncertainty in Brief Details' based on Laura's question 'What did you get?' [02:06–02:10]. The transcript shows David answered and Laura confirmed 'everybody got it' [02:20], resolving the uncertainty. The risk is therefore slightly over-stated."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

Based on the transcript, the main topic of the meeting was the kickoff for a new remote control design project. Laura, the project manager, stated they are "designing a new remote control" that is supposed to be "original, trendy, and user-friendly" [01:32–01:58]. She also outlined a three-stage iterative process of individual work followed by meetings [02:25–02:28].

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the provided transcript and meeting intelligence, no concrete decisions were explicitly made. The meeting intelligence also confirms that the decisions list is empty.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

The transcript does not contain any explicit action items or follow-ups.

Q4. What important unresolved questions or risks remain?

Based on the transcript, no unresolved questions or risks are mentioned. The meeting covers introductions, the project brief, the three-stage process, and an icebreaker activity, but does not identify any uncertainties, blockers, or risks.

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**Project Brief: Remote Control Design Kickoff**

**Problem**
The team is tasked with designing a new remote control that is original, trendy, and user-friendly, as outlined by Laura, the project manager [01:49–01:58].

**Approach**
The project will follow a three-stage iterative process, alternating between individual work and team meetings [02:25–02:28]. The team includes Laura (project manager), David (industrial designer), Andrew (marketing expert), and Greg (digital interface) [01:12–01:27].

**Decisions**
No explicit decisions are captured in the transcript.

**Risks**
- **Uncertainty in Brief Details**: Laura expressed uncertainty about what information team members had received via email, asking “What did you get?” [02:06–02:10]. David noted he only received a basic project announcement [02:12–02:14], suggesting potential gaps in shared understanding of the full brief.

**Next Steps**
- Begin the first stage of individual work as part of the iterative design process [02:25–02:28].
- The icebreaker activity (drawing favorite animals) is currently in progress and does not represent a formal project step [02:34–02:45].

## ami-es2002a-20min

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 8871 ms; verifier latency: 5573 ms
Counts: `{"draft_actions": 3, "draft_decisions": 0, "verified_actions": 3, "verified_decisions": 0, "filtered_actions": 3, "filtered_decisions": 0}`

### Summary
The meeting was a kick-off for designing a new remote control. Attendees introduced themselves and participated in an icebreaker activity where they drew their favorite animals and described their characteristics. The group then discussed project finances, noting the remote control will sell for 25 euros with a production cost cap of 12.50 euros, and considered the implications of international sales. The conversation shifted to design ideas, including combining multiple device controls, adding new functionalities like home lighting, and addressing common user frustrations such as losing the remote. The meeting concluded with the project manager outlining the next steps for individual work before the next meeting in 30 minutes.

### Actions
- Work on the actual work and design of the remote control | assignee: David | due: None | evidence: as the industrial designer you're going to be working on you know the actual work and design of it
- Work on user interface and technical functions | assignee: Craig | due: None | evidence: for our user interface technical functions i guess that's you know like what we've been talking about what it'll actually do
- Think about the requirements the remote control has to fulfill | assignee: Andrew | due: None | evidence: and and uh more to being executive you'll be just thinking about what it actually what you know what requirements it has to has to fulfill

### Decisions
- None explicit.

### Judge
```json
{
  "summary_accuracy": 8,
  "summary_coverage": 8,
  "action_precision": 7,
  "decision_precision": 10,
  "chat_grounding": 7,
  "overall": 8,
  "issues": [
    "Action item for Andrew is misassigned: transcript says 'more to being executive' but does not name Andrew; the MoM invents Andrew as owner.",
    "Chat answer for action items repeats the same owner error (assigns Andrew).",
    "Chat PM brief states 'no explicit decisions were captured' which is accurate, but the brief invents 'Find-me feature' as a design direction when it was only a question, not an agreed approach.",
    "Summary mentions 'adding new functionalities like home lighting' as a design idea, which is supported, but the chat brief treats it as an agreed design direction, which is a slight overstatement."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

The main topic of the meeting was the kick-off for designing a new remote control.

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the provided transcript and meeting intelligence, no concrete decisions were explicitly made. The meeting intelligence confirms the decisions list is empty.

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

Based on the transcript, the explicit action items and follow-ups are:

- **David** will work on the actual work and design of the remote control (as the industrial designer).
- **Craig** will work on user interface and technical functions.
- **Andrew** will think about the requirements the remote control has to fulfill.

Q4. What important unresolved questions or risks remain?

Based on the transcript, the following unresolved questions or risks remain:

- **International design implications**: Andrew raised the question of whether selling internationally would affect the design, mentioning DVD region codes, different keypad styles, and character sets for European languages. This was acknowledged but not resolved (around [09:11]).
- **Market positioning and price sensitivity**: Andrew questioned whether the 25-euro price point would appeal equally across different markets (e.g., London vs. Greece), noting that price sensitivity varies by region. This was left as an open consideration (around [09:44]).
- **Scope of functionality**: Andrew explicitly asked whether the remote control should be limited to television features or include functions beyond the TV (e.g., home lighting). The project manager said it was up to the team, and David noted production cost as a limiting factor, but no decision was made (around [17:27]).
- **Production cost constraint**: David pointed out that the production cost cap of 12.50 euros would limit how many features could be included, but no resolution was reached on how to balance features against cost (around [18:04]).

No explicit risks were identified in the transcript beyond these unresolved questions.

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**Project Brief: New Remote Control Design – Kick‑off Meeting**

**Problem / Opportunity**
- The team is tasked with designing a new remote control that is “original, trendy and user‑friendly” (01:49–01:58).
- The product must sell for **€25** with a production cost cap of **€12.50** (50% of selling price) and is aimed at an international market (07:52–08:08).
- Early discussion surfaced common user frustrations: losing the remote (15:03–15:09), juggling multiple remotes for different devices (13:14–13:21), and a desire for more streamlined, chic home‑tech objects (15:46–16:11).

**Approach / Design Directions Discussed**
- **Multi‑device consolidation:** Combine control of TV, satellite, VCR, DVD, CD, etc. into one remote (13:21–13:31).
- **Extra functionality:** Explore adding features beyond entertainment, such as home lighting control or personal viewing notes (13:59–14:14).
- **Find‑me feature:** Consider a beep‑on‑demand or similar locator function to solve the “lost remote” problem (15:15–15:20).
- **Form factor & materials:** Investigate touch‑screen interfaces and higher‑quality materials to make the remote more “chic” and streamlined (15:48–16:11).
- **International considerations:** Account for regional character sets, keypad layouts, and varying price sensitivity across markets (09:11–10:15).
- **Cost constraint:** All feature ideas must be evaluated against the €12.50 production cost cap (18:04–18:16).

**Decisions**
- No explicit decisions were captured in the meeting.

**Risks & Unresolved Questions**
- **Uncertain scope:** It is not yet decided whether the remote will be limited to television‑centric features or will expand into broader home‑control functions (17:41–17:57). The project manager indicated this is “up to us” (17:57).
- **Market positioning unclear:** The team does not know how a €25 remote compares to existing products or what “trendy” means in concrete design terms (11:02–11:42, 12:07–12:11).
- **International complexity:** Supporting multiple languages and regional standards may increase button count and design complexity, potentially conflicting with a streamlined aesthetic (09:33–09:44).
- **Cost vs. features trade‑off:** Adding functionality (e.g., touch screen, find‑me beeper, extra device codes) must fit within the €12.50 production budget; feasibility is not yet assessed (18:04–18:16).

**Next Steps (before next meeting in ~30 minutes)**
- **David (Industrial Designer):** Work on the physical design and form of the remote (16:28–16:33).
- **Craig (User Interface / Technical Functions):** Define what the remote will actually do, including UI and technical capabilities (16:37–16:45).
- **Andrew (Marketing / Requirements):** Think through the requirements the remote must fulfill, considering market positioning and user needs (16:46–16:56).
- **All:** Expect instructions via email; next meeting is the functional design stage (17:08–17:16).

**Uncertainty Note**
- The brief’s scope (TV‑only vs. multi‑device/home control) remains an open question. The team should clarify this early in the next session to align design, technical, and marketing workstreams.

## ami-en2001a-30min-mix

Provider/model: deepseek / deepseek-v4-pro
Draft latency: 13269 ms; verifier latency: 6325 ms
Counts: `{"draft_actions": 4, "draft_decisions": 3, "verified_actions": 2, "verified_decisions": 1, "filtered_actions": 2, "filtered_decisions": 1}`

### Summary
The team discussed Steve's feedback, which emphasized avoiding duplication of effort and prioritizing a working prototype by week six. They debated the data structure for the browser, considering whether to store importance measures at the word or utterance level, ultimately leaning toward utterance-level storage for simplicity and better skimming. The conversation covered how to handle the meeting series view, with ideas ranging from loading all data into memory to creating a virtual meeting structure if memory constraints are an issue. They also discussed integrating offline-calculated data with the NightXML framework, using its event model for synchronization, and handling search via an SQL database that feeds timestamps back to the NightXML manager. The team identified key data elements needed: utterances with weights, speaker information, topic segments, and timestamps, and planned to investigate NightXML's capabilities further.

### Actions
- Investigate NightXML code to understand data representation and compatibility | assignee: None | due: None | evidence: we then actually need to, well, go back first of all and look at night XML to see how far that which we want is compatible with that which night XML offers us
- Ask Jonathan about NightXML lazy loading and memory issues | assignee: Speaker 1 | due: None | evidence: I'll probably ask Jonathan about it.

### Decisions
- Search functionality will use an SQL database and feed timestamps back to NightXML | evidence: the searching leaves the whole framework, goes to the SQL database, and basically then gets just a time marker for where that is

### Judge
```json
{
  "summary_accuracy": 8,
  "summary_coverage": 8,
  "action_precision": 7,
  "decision_precision": 9,
  "chat_grounding": 8,
  "overall": 8,
  "issues": [
    "MoM summary is accurate but omits the explicit discussion about the prototype deadline being week six and the decision to prioritize the interface.",
    "Action item 'Investigate NightXML code' has no assignee in the MoM, which is correct per the transcript, but the MoM could note it was a general team task.",
    "Chat answer for 'What important unresolved questions or risks remain?' correctly identifies risks but invents 'Speaker 1 notes that NightXML fails when loading all data'—the transcript says 'we do get an error message with the project if we load everything', not specifically NightXML failing.",
    "Chat answer for 'Give a PM-ready brief' invents 'the team has seen out-of-memory errors with the full project load'—the transcript only mentions an error message, not specifically out-of-memory.",
    "Chat answer for 'What concrete decisions were explicitly made?' correctly identifies the SQL decision but fails to mention the implicit consensus to store importance at utterance level, which was a key discussion outcome."
  ]
}
```

### Chat

Q1. What was the main topic of this meeting?

The main topic of this meeting was settling on the data structure for the meeting browser, specifically deciding what data to store and how to represent it (e.g., at the utterance vs. word level) to support features like skimming, search, and the meeting series view, while considering integration with the NightXML framework.

Q2. What concrete decisions were explicitly made? If none, say none.

Based on the provided transcript and the meeting intelligence, the only explicit decision captured is:

- **Search functionality will use an SQL database and feed timestamps back to NightXML** (evidence: "the searching leaves the whole framework, goes to the SQL database, and basically then gets just a time marker for where that is").

Q3. List only explicit action items or follow-ups, with owners only if the transcript gives them.

Based on the transcript, the explicit action items or follow-ups are:

- Investigate NightXML code to understand data representation and compatibility (no owner specified).
- Ask Jonathan about NightXML lazy loading and memory issues (owner: Speaker 1).

Q4. What important unresolved questions or risks remain?

Based on the transcript, the following unresolved questions and risks are present:

- **Memory constraints for loading the entire meeting series**: The team is uncertain whether loading all utterance data for a whole meeting series into memory is feasible. Speaker 1 notes that NightXML fails when loading all data and that 50–100 MB in RAM is "quite big" [18:25–18:46]. This remains an open question, with Speaker 1 planning to ask Jonathan about lazy loading and memory issues [20:09–20:13].

- **Compatibility with NightXML**: The team has not yet investigated whether their desired data structure (utterances with weights, speakers, topic segments, timestamps) is compatible with NightXML’s representation. Speaker 1 states they need to "look at night XML to see how far that which we want is compatible with that which night XML offers us" [01:54–01:58].

- **Utterance-level vs. word-level storage**: While the team leans toward storing importance measures at the utterance level, the final decision is not fully resolved. Concerns remain about how to handle unimportant words within utterances and whether word-level timestamps can be aggregated to utterance-level timestamps [13:00–13:10, 14:51–15:28].

- **Audio playback of selected utterances**: The feasibility of buffering and concatenating audio segments in memory for skimming is still uncertain. Speaker 3 suggests it "might be like an hour or two's work," but no one has confirmed the capability of the audio player [15:58–16:30].

- **Utterance IDs in NightXML**: There is confusion about whether utterances have unique IDs in the existing NightXML data. Speaker 2 and Speaker 4 indicate that only individual words have IDs, which could complicate linking external data to utterances [27:00–28:00].

Q5. Give a PM-ready brief with problem, approach, decisions, risks, and next steps. Mark uncertainty.

**Brief**

**Problem**
The team needs to settle on a data structure and integration plan for the meeting browser to deliver a working prototype by week six, while avoiding duplication of effort and staying compatible with the NightXML framework.

**Approach**
- Prioritize a working prototype by integrating pre‑annotated data and simple baselines, focusing on the interface first.
- Store importance measures at the **utterance level** (not word level) to reduce memory, simplify audio skimming, and produce more coherent summaries.
- For the meeting‑series view, either load all utterance‑level data into memory (if feasible) or create a virtual meeting structure that treats meetings as segments and segments as utterances, reusing the same display logic.
- Use NightXML’s event model for synchronisation across display elements; search will leave the NightXML framework, query an SQL database, and feed timestamps back to the NightXML manager to trigger updates.
- Key data elements per utterance: weight/importance, speaker, topic segment, and timestamps.

**Decisions**
- Search functionality will use an SQL database and feed timestamps back to NightXML (explicit decision from meeting intelligence).

**Risks & Uncertainties**
- **Memory feasibility**: It is unclear whether loading all utterance data for a 70‑hour meeting series into NightXML will work; the team has seen out‑of‑memory errors with the full project load. Lazy loading may help, but this needs verification.
- **NightXML compatibility**: The team has not yet examined the NightXML code to confirm how data representation, utterance IDs, and lazy loading work.
- **Utterance‑level storage**: The impact of averaging word‑level importance to utterance level (e.g., handling stop words) is not fully resolved; a threshold or pre‑filtering may be needed.
- **Audio skimming**: Building an in‑memory audio concatenation class is assumed to be straightforward (“an hour or two’s work”), but this has not been prototyped.

**Next Steps**
1. **Investigate NightXML** – examine the code to understand data representation, utterance IDs, and lazy‑loading capabilities.
2. **Ask Jonathan** (Speaker 1) about NightXML lazy loading and memory issues.
3. **Define utterance‑level data format** – agree on the exact fields (utterance ID, weight, speaker, topic segment, timestamps) and how they map to NightXML files.
4. **Prototype the interface** – wire together pre‑annotated data and baseline components to have a working demo by week six.
