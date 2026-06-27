# Sentinel

## The Coverage Intelligence Layer for Regulated Engineering

A QA and engineering leadership layer that continuously maps requirements, code, tests, risk, and ownership, then turns the highest-confidence coverage gaps into verified test changes and audit-ready evidence.

| Field | Detail |
|---|---|
| Prepared for | Stryker Engineering Leadership |
| Document type | Solution Brief & Leadership Pitch |
| Version | v1.1, May 2026 |
| Audience | VP of Engineering, Head of QA, CTO, Security & Compliance Leadership |
| Reading time | Executive summary: 2 minutes; full document: 12 minutes |

## Contents

1. Executive Summary
2. The Problem We Are Solving
3. What Sentinel Is
4. Platform Capabilities
5. How It Works
6. How Sentinel Compares
7. Security, Compliance & Deployment
8. Business Case & Expected Outcomes
9. Adoption Roadmap
10. Risks & Mitigations
11. Engagement Model & Next Steps
12. Appendix: Technical Deep-Dive
13. Selected Research & Source Notes

## 1. Executive Summary

AI has made code creation faster. It has not made coverage governance automatic.

Engineering teams now ship more code, across more frameworks and product surfaces, with more AI assistance than before. QA teams are under pressure to keep up, but much of their time still goes into mechanical coordination: turning requirements into test cases, checking what is covered, filing tickets, chasing owners, and re-validating fixes. The highest-value QA work is not writing one more test manually. It is knowing which risks are uncovered, why they matter, and what evidence proves they have been addressed.

Sentinel is built for that leadership problem.

Sentinel reads code, requirements, and existing tests in parallel. It builds a live coverage map, identifies high-confidence gaps, generates test changes, verifies them through quality gates, and routes each action to the developer or team that owns the code. The result is not just more tests. The result is a continuously updated view of software quality risk: what should exist, what exists, what is tested, what changed, who owns it, and what evidence supports the decision.

For Stryker, this matters because software quality is not only an engineering concern. It is connected to product reliability, regulatory evidence, cybersecurity, audit readiness, and executive quality governance. Stryker publicly emphasizes quality as a leadership-level operating principle, with broad ISO 13485 certification, regular independent audits, and quality data reviewed at executive levels. Sentinel is designed to fit that reality: improving coverage while producing evidence that engineering, QA, security, and quality leaders can inspect.

### Leadership outcomes

| Outcome | What Sentinel improves |
|---|---|
| Quality risk visibility | A live map of requirements, code, tests, ownership, and uncovered behaviors. |
| Evidence generation | Each surfaced gap includes rationale, source links, test evidence, verification results, and acceptance or dismissal history. |
| Engineering throughput | QA and developers spend less time searching for gaps and more time resolving the right ones. |
| Audit readiness | Coverage decisions are traceable, reviewable, and exportable instead of scattered across chats, tickets, and tribal knowledge. |

## 2. The Problem We Are Solving

### QA is the bottleneck nobody talks about

Every engineering organization knows the pattern. Velocity is up. Surface area is up. Frameworks have multiplied. Mobile, web, backend, APIs, integrations, and AI-assisted changes all need coverage strategies. But QA capacity does not scale linearly with that complexity.

The cost shows up in three places:

| Escaped defects | Slow releases | Burnt-out teams |
|---|---|---|
| Bugs that should have been caught at the test layer surface in production or late validation. | Release trains slow down because regression cycles and coverage reviews take days. | QA and developers spend too much time on repetitive coordination instead of judgment-heavy quality work. |

### Why now

AI-assisted development makes this problem sharper. Research from DORA has repeatedly found that AI can improve individual productivity, but the benefits depend on strong engineering systems. Without those systems, AI can amplify instability, rework, and delivery friction. For regulated engineering, that means the central question is not only "Can we write code faster?" It is:

> Can we continuously prove that the right requirements, risks, code paths, and product behaviors are covered as development accelerates?

The regulatory context also matters. FDA's Quality Management System Regulation became effective on February 2, 2026 and incorporates ISO 13485:2016 into 21 CFR Part 820. FDA's software assurance guidance also emphasizes risk-based confidence in automation used for production or quality systems. In that environment, quality evidence needs to be structured, repeatable, and reviewable.

### Why existing tools have not fully solved this

The current generation of AI and QA tools solves important slices of the problem:

- Low-code and agentic test automation tools help teams create and maintain UI, API, mobile, and end-to-end tests faster.
- Visual AI tools help detect visual regressions and cross-browser differences.
- Code-level tools can generate unit tests for specific language ecosystems.
- AI code review tools help inspect pull requests and enforce development standards.
- Test management platforms help organize plans, execution, and reporting.

These tools are useful. Sentinel is not positioned as a rip-and-replace alternative to them.

The gap Sentinel addresses is higher level: most tools help teams create, run, or review tests, but they do not continuously reconcile requirements, code, tests, risk signals, and ownership into one action queue.

Sentinel answers the harder leadership question:

> Which gaps in our coverage matter most right now, what evidence exists, and who should act next?

## 3. What Sentinel Is

Sentinel is an autonomous coverage intelligence platform deployed alongside your engineering stack. It connects to three sources of truth:

- Your code
- Your requirements and design intent
- Your existing tests and test results

It then runs a continuous loop:

| Reads | Finds gaps | Writes tests | Pings the owner |
|---|---|---|---|
| Services, APIs, screens, requirements, design behaviors, and existing tests. | Untested code paths, required-but-unverified behaviors, stale coverage, and high-risk change areas. | Generates test code and runs it through verification before it reaches a developer. | Routes verified work to the right owner with context, evidence, and a path to merge or dismiss. |

Sentinel is not a CI replacement. It does not own your test runners, deployment pipelines, or monitoring. It plugs into what teams already use: Azure Pipelines, GitHub Actions, GitLab CI, Jenkins, Jira, Azure DevOps Boards, Slack, Teams, and existing quality systems.

Think of Sentinel as the layer between requirements, code, tests, and ownership. It automates the mechanical work so QA and engineering leaders can focus on judgment, risk, and release confidence.

### Three principles that shape the product

1. **Coverage as a continuous map, not a static number**
   Sentinel maintains a live overlay of requirements, code, tests, risk, and ownership. Coverage becomes something leadership can inspect and govern, not just a percentage on a dashboard.

2. **AI where it earns its keep, deterministic code everywhere else**
   Most of Sentinel is deterministic pipelines: parsers, indexes, scoring, verification, routing logic, and audit logging. AI is reserved for the places where it adds real value: interpreting messy requirements and generating codebase-aware tests.

3. **A few high-confidence gaps beat a noisy backlog**
   Developer attention is scarce. Sentinel scores every gap using risk, change velocity, blast radius, defect history, and business priority. It surfaces the top items and keeps lower-signal findings tracked but quiet.

## 4. Platform Capabilities

Sentinel covers the surfaces that matter in modern engineering organizations. Exact integrations are validated and prioritized during the pilot so the first deployment proves value without over-scoping.

| Capability area | Sentinel approach |
|---|---|
| Source control | Azure DevOps Repos, GitHub, GitLab, and Bitbucket through read-only service access. Indexed continuously or on push. |
| Requirement systems | Jira, Confluence, Azure DevOps Boards, Notion, Figma, and additional regulated requirement systems through adapters. |
| Backend coverage | Java, Kotlin, Python, Go, Node/TypeScript, C#, and other common service stacks. Unit, integration, and contract-test generation based on framework detection. |
| Web/frontend coverage | React, Vue, Angular, and Svelte. Component, journey, accessibility, and visual-regression coverage where relevant. |
| Mobile coverage | Native iOS, native Android, React Native, and Flutter through framework-specific test strategies. Mobile scope should be phased because iOS and Android require different validation paths. |
| Figma awareness | Component states and design intent can become testable behaviors: default, hover, error, loading, empty, disabled, and accessibility states. |
| CI integration | Sentinel hands generated tests to the CI/CD systems teams already run. It does not replace CI ownership. |
| Notification channels | Slack, Microsoft Teams, Jira, Azure DevOps, GitHub, and PR comments. Routing uses code ownership, recent change history, service catalog data, and team mapping. |
| Verification gates | Generated tests must build, run, pass, remain stable, add coverage, and catch seeded defects before they are routed for review. |
| Deployment | Target deployment is single-tenant and tenant-contained, using Stryker-approved Azure infrastructure and security controls. Exact model, region, networking, and retention configuration are validated during pilot setup. |
| Observability and audit | Prompts, responses, gaps, generated tests, verification results, routing decisions, and human dispositions can be stored as an evidence trail and streamed to customer-controlled monitoring/SIEM systems. |

## 5. How It Works

Sentinel runs a six-stage loop. The first three stages run continuously in the background. The last three trigger when a meaningful gap is found.

| Stage | Name | What happens | Output evidence |
|---|---|---|---|
| 01 | Ingest | Parse code, requirements, design sources, existing tests, and recent change history. | Code/requirement/test index snapshot. |
| 02 | Map coverage | Overlay code, requirements, tests, and behaviors to identify covered and uncovered regions. | Requirement-code-test trace links. |
| 03 | Prioritize | Score gaps by risk, change velocity, blast radius, defect history, and business priority. | Risk score and explanation. |
| 04 | Write & verify | Generate candidate tests and run them through quality gates. | Test PR plus verification report. |
| 05 | Ping the owner | Route work to the right developer or team using ownership signals. | Jira/ADO ticket, Slack/Teams note, or PR comment with owner rationale. |
| 06 | Close the loop | Track accepted, edited, dismissed, and merged outcomes. | Decision history and updated coverage map. |

### What runs on AI, and what does not

Most of Sentinel is not AI. Deterministic systems handle parsing, indexing, dependency mapping, scoring, routing signals, verification, storage, and audit logs. AI is used where language and code generation genuinely benefit from model reasoning:

- Parsing ambiguous requirements into structured behaviors
- Translating design intent into testable acceptance criteria
- Generating codebase-aware tests that match local style and frameworks
- Drafting developer-facing explanations that are concise and useful

This architecture matters for security and cost. Sentinel should be easy to explain to platform and security teams because the model is only one part of the system, not the system itself.

## 6. How Sentinel Compares

The QA tooling market is crowded because "AI for QA" now means many different things. Sentinel is deliberately not framed as a replacement for every testing tool. It sits above them as a coverage intelligence and evidence layer.

| Category | Examples | Where they are strong | Where Sentinel is different |
|---|---|---|---|
| Low-code and agentic E2E automation | Testim, mabl, Functionize, Testsigma, Katalon | Fast authoring, self-healing UI tests, cloud execution, web/mobile/API flows. | Sentinel starts from requirements, code, tests, risk, and ownership, then identifies which gaps matter most. |
| Visual AI and cross-browser validation | Applitools and similar tools | Visual assertions, accessibility, cross-device UI evidence. | Sentinel decides where visual evidence is missing and records the evidence trail. It can integrate with specialist visual engines instead of replacing them. |
| Code-level unit-test generation | Diffblue and language-specific generators | Autonomous test generation for specific ecosystems, especially Java/Kotlin. | Sentinel is broader: multi-language, multi-surface, and tied to requirements, risk, and ownership. |
| PR review and AI code governance | GitHub Copilot, Qodo, CodeRabbit | Reviewing diffs, enforcing standards, suggesting fixes. | Sentinel reasons beyond the current PR. It finds untested required behavior and stale risk areas that may not appear in a diff. |
| Test management and quality analytics | Tricentis qTest/SeaLights, Parasoft, Azure DevOps Test Plans | Test planning, execution tracking, traceability, quality dashboards. | Sentinel supplies a live coverage graph and action queue that can feed existing quality systems. |

### The headline difference

Other tools help teams create, run, or review tests faster.

Sentinel helps leadership know whether the right things are covered, why a gap matters, what evidence exists, and who owns the next action.

That shift from test authoring to coverage governance is what makes Sentinel a platform decision rather than a point productivity tool.

## 7. Security, Compliance & Deployment

Sentinel is designed around a simple operating principle: source code, prompts, generated artifacts, and quality evidence should stay under customer-controlled infrastructure and governance.

For Stryker, the pilot should validate the exact deployment details before production-scale rollout: Azure region, model endpoint, networking, retention policy, identity model, logging, SIEM integration, and egress controls.

| Layer | Target posture |
|---|---|
| Network | Tenant-contained deployment using customer-approved Azure networking patterns, with egress policy reviewed during pilot setup. |
| Inference | Enterprise-approved model endpoint. Region, retention, partner-model terms, and private-network options validated before production use. |
| Identity | Microsoft Entra ID and workload identity patterns. No standing personal credentials. |
| Repository access | Read-only by default. Scope limited to pilot repositories and approved branches. |
| Storage | Customer-controlled stores for indexes, prompts, generated artifacts, verification results, and audit logs. |
| Audit | Every surfaced gap should include traceable inputs, generated output, verification result, routing rationale, and human disposition. |
| Residency | Region and data-flow design reviewed with Stryker security and compliance during pilot. |
| Compliance alignment | Designed to support SOC 2, ISO 27001, HIPAA, GDPR, and regulated engineering evidence workflows; final mapping depends on Stryker control requirements. |

### Why this matters for procurement

Many QA and developer tools are SaaS-first. That can be appropriate for some workflows, but it creates friction when source code, regulated product evidence, or confidential design history are involved. Sentinel's target architecture is different: deploy close to the engineering environment, integrate with existing identity and monitoring, and produce a customer-controlled evidence trail.

From the security team's point of view, Sentinel should look like one more governed workload, not a new uncontrolled vendor surface.

## 8. Business Case & Expected Outcomes

Sentinel pays back along three vectors:

- Reclaimed QA capacity
- Higher confidence coverage, faster
- Fewer late-stage or escaped defects

The exact numbers should be proven in a pilot rather than asserted in advance. The four-week pilot should be structured around metrics that Stryker and Sentinel agree on before kickoff.

| Metric | Why leadership cares | Pilot measurement |
|---|---|---|
| Accepted high-confidence gaps | Measures signal quality, not alert volume. | Merged unchanged, merged with edits, dismissed with reason. |
| Requirement-code-test traceability uplift | Measures audit and coverage visibility. | Before/after trace map for the pilot repository. |
| Coverage on risk-ranked surfaces | Measures whether Sentinel improves the areas that matter. | Coverage delta on selected services, APIs, screens, or product behaviors. |
| Time from gap detection to action | Measures operating speed. | Median and 90th percentile from gap creation to merge or dismissal. |
| QA/developer review effort | Measures capacity reclaimed. | Baseline survey plus ticket/PR timestamps. |
| False-positive rate | Measures trust. | Dismissed as invalid or not worth action. |

### How we propose to prove the case

We do not want Stryker to take the business case on faith. At day 30, the data should say yes, no, or tune before expansion.

Pilot success metrics:

- Coverage uplift on the pilot repository
- Number of high-confidence gaps surfaced
- Developer acceptance rate
- Time from gap detection to merged fix or documented dismissal
- QA hours reclaimed
- Quality and security stakeholder confidence in the evidence trail

### What we need from Stryker

- One pilot team and repository with active development and an engaged tech lead
- Read-only access to the selected repo and relevant requirement source
- A platform/security contact for Azure deployment review
- Agreement on success metrics and stop/go criteria before kickoff
- One QA or quality stakeholder to review whether the generated evidence is useful

### Suggested stop/go criteria

| Decision point | Recommended action |
|---|---|
| Developer acceptance is strong and evidence is useful | Expand to additional teams and surfaces. |
| Generated tests require too much manual repair | Pause generation and continue in mapping/evidence mode while tuning. |
| Requirements are too ambiguous or stale | Route ambiguity back to product/QA instead of generating low-confidence tests. |
| Security cannot validate data flow | Limit pilot to non-sensitive repositories or pause rollout. |

## 9. Adoption Roadmap

Sentinel rolls out in three phases. Each phase has clear entry criteria, exit criteria, and success metrics.

| Phase | Days 0-30: Pilot | Days 31-60: Expand | Days 61-90: Scale |
|---|---|---|---|
| Scope | One team, one repository, selected for active development and low operational friction. | Two to four additional teams. Add a second surface such as frontend or mobile if the pilot was backend. | Organization-wide rollout across priority engineering teams and product surfaces. |
| Activities | Deploy in Azure, index code and requirements, create baseline coverage map, surface first gaps in week two. | Tune prioritization, add more integrations, validate evidence export, build first prevention case study. | Standardize operating cadence, integrate with service catalog/SIEM/quality systems, define governance ownership. |
| Success metrics | Coverage uplift, accepted gaps, time-to-action, QA effort reclaimed, evidence usefulness. | Stable or improving acceptance rate, broader traceability, first measurable defect-prevention story. | Defined org-wide coverage baseline, ongoing dashboard, internal owner team, audit-ready evidence flow. |
| Stryker effort | One tech lead, one QA engineer, one PM/product contact, one platform/security contact. | One coordinator plus participating team leads. | Steady-state owner team for governance and tuning. |

## 10. Risks & Mitigations

A platform decision deserves a clear-eyed view of what could go wrong.

| Risk | How Sentinel mitigates it |
|---|---|
| Generated tests are low quality or flaky | Verification gates require tests to build, run, pass, remain stable, add coverage, and catch injected defects before routing. |
| Developer fatigue from too many notifications | Sentinel limits surfaced gaps and optimizes for high-confidence action, not volume. |
| Requirements are ambiguous or stale | Sentinel flags ambiguity and routes it back to product/QA instead of generating false certainty. |
| AI-generated tests encode current behavior instead of intended behavior | Generated tests must link back to requirements, design intent, or explicit code-risk rationale. |
| Source code leaves the intended perimeter | Pilot includes security review of networking, model endpoint, retention, storage, and logging before production use. |
| Tool overlap creates adoption resistance | Sentinel integrates with existing QA and DevOps systems rather than replacing them. |
| Vendor lock-in on a single model provider | The architecture should keep deterministic graph, adapters, prompts, and verification logic model-portable. |
| Inference cost grows unpredictably | AI usage is metered by worker and repository. Budget caps and team-level quotas can be configured. |
| Internal team cannot operate the platform | Worker services, evidence formats, and runbooks should be documented as part of rollout. |

## 11. Engagement Model & Next Steps

The recommended next step is a focused pilot, not a broad deployment.

### Pilot plan

1. Select one repository and one product/team owner.
2. Confirm security and deployment constraints.
3. Connect source control, requirement source, CI, and notification channel.
4. Build the baseline coverage map.
5. Surface a limited number of high-confidence gaps.
6. Generate and verify test changes.
7. Measure acceptance, effort, traceability, and evidence quality.
8. Decide whether to expand, tune, or stop.

### What Stryker receives at the end of the pilot

- Baseline and updated coverage map
- Top risk-ranked gaps and reason codes
- Example generated test PRs
- Verification reports
- Owner-routing rationale
- Accepted/dismissed/merged log
- Security/data-flow summary
- Model usage and cost report
- Recommendation memo: expand, tune, or stop

## 12. Appendix: Technical Deep-Dive

### A.1 Architecture overview

Sentinel runs as a set of worker services coordinated by an orchestrator, with persistent stores for code index, requirements, generated artifacts, verification results, and audit history. The system is designed to sit beside existing CI rather than replace it.

At a high level:

- **Orchestrator:** coordinates ingestion, mapping, scoring, generation, verification, routing, and feedback.
- **Code index:** parses source code, dependencies, ownership signals, and tests.
- **Requirements store:** normalizes requirements, acceptance criteria, design states, and linked business priorities.
- **Coverage graph:** maps requirements to code paths, tests, owners, and risk signals.
- **Generator:** creates test candidates when a high-confidence gap is found.
- **Verifier:** runs quality gates before anything reaches a developer.
- **Notifier:** routes the work to the right owner through existing developer workflows.
- **Audit store:** records inputs, outputs, decisions, and evidence.

### A.2 AI usage breakdown

| Worker | AI usage | What it does |
|---|---|---|
| Ingester | None | Parses syntax, resolves types, and builds dependency graphs using deterministic tooling. |
| Prioritizer | None | Scores gaps using explainable weights: risk, churn, blast radius, defects, and business priority. |
| Verifier | None | Runs containerized build/test/coverage/mutation checks. |
| Mapper | Hybrid | Uses deterministic matching first, semantic similarity second, and LLM reasoning only for ambiguous mappings. |
| Notifier | Hybrid | Routing is deterministic; human-readable message copy may be generated. |
| Requirements Parser | AI | Converts messy requirements, tickets, and design notes into structured behaviors. |
| Generator | AI | Writes candidate tests that fit the codebase style and framework. |

### A.3 The six verification gates

| # | Gate | What it checks |
|---|---|---|
| 1 | Builds | Compiles cleanly. Imports resolve. No syntax issues. |
| 2 | Runs | Executes end to end with mocks and fixtures wired correctly. |
| 3 | Passes today | Green against current code. The test is a baseline, not a bug report. |
| 4 | Stable | Same result on repeated runs. Flaky tests do not ship. |
| 5 | Adds coverage | Hits behavior or code paths not covered by existing tests. |
| 6 | Catches bugs | Seeded defects or negative checks prove the test detects meaningful failure. |

### A.4 Frontend coverage strategy

Frontend gaps split into three categories:

- **Behavioral gaps:** component tests and user-journey tests validate expected behavior.
- **Visual gaps:** Storybook, visual snapshots, and pixel comparison validate states such as default, hover, error, loading, and empty.
- **Experience gaps:** journey scripts and accessibility checks validate loading, error, keyboard, and assistive-technology behavior.

### A.5 Mobile coverage strategy

Mobile is phased carefully because iOS and Android require different validation approaches.

- Native iOS: XCTest, XCUITest, and snapshot testing
- Native Android: JUnit, Robolectric, Espresso, and screenshot/golden testing
- React Native: Jest, Detox, or Maestro depending on stack maturity
- Flutter: flutter_test, integration_test, and golden tests

The generator detects framework signals from the repository and proposes the least disruptive test strategy first.

## 13. Selected Research & Source Notes

These sources support the positioning and should be cited selectively in external versions:

- Stryker Global Quality: https://www.stryker.com/us/en/about/global-quality.html
- Stryker Mako SmartRobotics: https://www.stryker.com/us/en/joint-replacement/systems/Mako_SmartRobotics_Overview.html
- FDA Quality Management System Regulation: https://www.fda.gov/medical-devices/postmarket-requirements-devices/quality-management-system-regulation-qmsr
- FDA Computer Software Assurance guidance: https://www.fda.gov/regulatory-information/search-fda-guidance-documents/computer-software-assurance-production-and-quality-management-system-software
- FDA Cybersecurity in Medical Devices guidance: https://www.fda.gov/regulatory-information/search-fda-guidance-documents/cybersecurity-medical-devices-quality-management-system-considerations-and-content-premarket
- DORA 2024 report: https://dora.dev/research/2024/dora-report/
- DORA 2025 report: https://dora.dev/research/2025/dora-report/
- GitLab Global DevSecOps report: https://about.gitlab.com/resources/developer-survey/
- Tricentis Testim: https://www.tricentis.com/products/test-automation-web-apps-testim
- mabl: https://www.mabl.com/
- Functionize: https://www.functionize.com/
- Testsigma: https://testsigma.com/
- Katalon: https://katalon.com/
- Applitools: https://applitools.com/
- Diffblue Cover: https://cover-docs.diffblue.com/get-started/what-is-diffblue-cover
- GitHub Copilot test writing: https://docs.github.com/en/copilot/tutorials/write-tests
- GitHub Copilot code review: https://docs.github.com/en/copilot/how-tos/use-copilot-agents/request-a-code-review/use-code-review
- Qodo code review: https://docs.qodo.ai/code-review
- CodeRabbit docs: https://docs.coderabbit.ai/
- Parasoft: https://www.parasoft.com/

---

End of document. Sentinel · v1.1 · May 2026
