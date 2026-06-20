# Sentinel

## The Autonomous QA Layer for Modern Engineering

A QA intelligence layer that reads your codebase, finds coverage gaps, writes verified tests, and routes the right action to the right developer.

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

## 1. Executive Summary

With AI as an assist, engineering teams today ship more code, in more places, with more frameworks than ever before. QA has not scaled at the same pace. Too much time still goes into mechanical work: writing test cases from PRDs, checking what is covered, filing tickets, chasing developers, and re-validating fixes. Too little time is left for the high-judgement work that actually prevents defects.

Sentinel gives QA and engineering teams an autonomous layer for coverage intelligence.

It reads your codebase, requirements, and existing tests in parallel; finds the gaps where coverage is missing; writes tests automatically; verifies them through six quality gates; and routes each one to the developer who actually owns the code. The target deployment is single-tenant and customer-controlled, aligned with Stryker's Azure, identity, monitoring, and security patterns.

For Stryker, the value is not simply "more tests." The value is knowing what should be covered, what is actually covered, which gaps matter most, who owns them, and what evidence exists for leadership, quality, security, and audit review.

## 2. The Problem We Are Solving

### QA is the bottleneck nobody talks about

Every engineering organisation knows the pattern. Velocity is up. Surface area is up. Frameworks have multiplied. Mobile, web, backend, APIs, integrations, and AI-assisted code all need their own coverage strategy. But QA capacity has stayed roughly flat, and the cost of that gap shows up in three places.

| Escaped defects | Slow releases | Burnt-out teams |
|---|---|---|
| Bugs that should have been caught at the test layer end up surfacing late or in production. | Release trains slow down because regression cycles and coverage checks take days. | QA and developers spend too much time on repetitive handoffs instead of higher-value quality work. |

This problem is sharper in regulated engineering. Stryker operates in an environment where quality evidence, design control, software assurance, cybersecurity, and audit readiness matter. FDA's updated Quality Management System Regulation became effective in 2026 and incorporates ISO 13485:2016 into 21 CFR Part 820. That makes traceable, risk-based software quality evidence more important, not less.

### Why existing tools haven't fixed this

The current generation of "AI for QA" tools generally solves pieces of the problem. Low-code and agentic test automation platforms such as Testim, mabl, Functionize, Katalon, and Testsigma help teams create and maintain UI, API, mobile, and end-to-end tests faster. Code-generation copilots help individual developers write tests or review pull requests. Test-management platforms help organise plans, runs, and reporting.

These tools are useful, but they usually do not answer the system-level coverage question: across requirements, code, tests, ownership, risk, and recent change history, which gaps matter most right now, and who should fix them?

That is the question Sentinel is built to answer.

## 3. What Sentinel Is

Sentinel is an autonomous QA platform deployed inside your own cloud environment. It connects to three sources of truth: your code, your requirements, and your existing tests, and runs a continuous loop:

| Reads | Finds gaps | Writes tests | Pings the owner |
|---|---|---|---|
| Every service, API, screen, requirement, and existing test. | Code paths nobody tests, requirements nobody implemented, behaviours nobody verified. | Generates test code, then runs it through a six-gate verifier before any human sees it. | Routes the verified test to the right developer with a ticket, PR comment, or team notification. |

### Three principles that shape the product

1. **Coverage as a continuous map, not a number.** Sentinel maintains a live, three-way overlay of requirements, code, and tests. Coverage is something you watch evolve, not a static percentage on a dashboard.

2. **AI where it earns its keep, plain code everywhere else.** Most of Sentinel is deterministic pipelines: fast, explainable, and easy to audit. AI is reserved for the places where it actually wins: parsing messy requirements and writing test code that fits your codebase's style.

3. **Five high-confidence gaps beat fifty noisy ones.** Developer attention is the scarcest resource in the system. Sentinel scores every gap on multiple signals and surfaces only the top of the queue. The rest stays tracked but silent.

### Where Sentinel fits in your stack

Sentinel is not a CI replacement. It does not own your test runners, deployment pipelines, or monitoring. It plugs into what you already use: Azure Pipelines, GitHub Actions, GitLab CI, Jenkins, and existing ticketing or notification systems. It complements your QA team rather than replacing them. Think of it as the layer that automates the mechanical work between code, requirements, and tests, so your humans can focus on the parts that need humans.

## 4. Platform Capabilities

Sentinel is designed for the full surface area of a modern engineering organisation. The matrix below summarises core capabilities and integrations that can be validated and prioritised during the pilot.

### Capability matrix

| Capability area | What's supported |
|---|---|
| Source control | Azure DevOps Repos, GitHub, GitLab, Bitbucket. Read-only access via service accounts. Indexed continuously or per push. |
| Requirement systems | Jira, Confluence, Notion, Figma, Azure DevOps Boards. Adapter layer accommodates additional sources, including regulated requirement systems such as Jama Connect, on request. |
| Backend coverage | Java, Kotlin, Python, Go, Node/TypeScript, C#, Ruby. Unit, integration, and contract tests. Framework-aware: Spring Boot, FastAPI, Express, .NET, Rails, and similar stacks. |
| Web / frontend | React, Vue, Angular, Svelte. Component tests, E2E tests, visual regression, and accessibility checks where relevant. |
| Mobile coverage | Native iOS, native Android, React Native, and Flutter. Mobile rollout should be phased because iOS and Android require different validation approaches. |
| Figma awareness | Component variants in Figma: default, hover, error, loading, empty, disabled. Design intent and engineering reality stop drifting apart silently. |
| CI integration | Azure Pipelines, GitHub Actions, GitLab CI, Jenkins, CircleCI. Sentinel never owns test execution; it hands generated tests to the CI you already run. |
| Notification channels | Slack, Microsoft Teams, Jira tickets, GitHub/Azure DevOps PR comments. Routing uses Git blame, CODEOWNERS, recent PR activity, and service catalog mapping. |
| Verification gates | Six gates run on every generated test before it surfaces: builds cleanly, runs end-to-end, passes against current code, remains stable, adds new coverage, catches injected bugs. |
| Deployment | Single-tenant deployment aligned with Stryker-approved Azure patterns. Model endpoint, region, retention, and networking controls are validated during pilot setup. |
| Observability & audit | Prompts, responses, gaps, generated tests, verification results, and notifications are recorded as audit evidence and can be streamed to customer-controlled monitoring or SIEM systems. |

## 5. How It Works

Sentinel runs a six-stage loop. The first three stages happen continuously in the background. The last three trigger when a gap is found.

### The six stages

| Stage | Name | What happens |
|---|---|---|
| 01 | Ingest | Parse code with Tree-sitter and Language Server Protocol. Read requirements from Jira, Confluence, Figma, Notion, or Azure DevOps. Read existing tests for what they actually exercise. Build a unified graph of the system. |
| 02 | Map coverage | Overlay code, requirements, and tests. Surface four regions: required-but-not-built, built-but-not-required, built-and-required-but-not-tested, and fully covered. |
| 03 | Prioritise | Score every gap on five signals: risk, change velocity, blast radius, defect history, and business priority. Surface only the top three to five per developer per week. |
| 04 | Write & verify | Generate test code candidates, reject weak candidates, and run the survivor through six verification gates. The quality filter is intentionally strict so developers see fewer, better suggestions. |
| 05 | Ping the dev | Combine routing signals such as Git blame, CODEOWNERS, recent PRs, and service catalog data to find the right owner. Send a ticket, PR comment, or Slack/Teams notification. |
| 06 | Close the loop | Re-run verification on merged code. Update the coverage map. Record what was accepted, dismissed, or rewritten, so every interaction improves future suggestions. |

### What runs on AI, and what doesn't

Most of Sentinel is not AI. The AI parts are the few places where AI is the right answer. Of seven worker services, only two are full AI agents, three are pure deterministic code, and two are hybrids where AI handles the last layer of ambiguity.

The result is a system that is fast, explainable, measurable, and straightforward to explain to a security team. Inference cost should be measured during the pilot by repository size, scan frequency, model choice, and acceptance rate rather than assumed up front.

## 6. How Sentinel Compares

The QA tooling market is crowded. Most tools fit into one of four buckets: low-code/agentic test automation, visual testing, code-generation copilots, or test-management platforms. Sentinel is not trying to replace all of them. It sits a layer above, reasoning about coverage at the system level.

### Where Sentinel sits versus the alternatives

| Capability | Sentinel | Testim / mabl | Functionize / Katalon / Testsigma | Diffblue | Copilot + DIY |
|---|---|---|---|---|---|
| Reasons about coverage at system level | Yes | Limited | Limited | Partial | No |
| Reads requirements and design context | Yes | Limited | Limited | No | Limited |
| Multi-surface: backend, web, mobile | Yes | Web/mobile focused | Web/API/mobile focused | JVM focused | Per developer |
| Single-tenant / customer-controlled deployment | Yes | SaaS-first | SaaS-first | Local/on-prem options | Varies |
| Source-code exposure controlled by customer | Yes | Varies | Varies | Yes | Varies |
| Routes to right developer automatically | Yes | Limited | Limited | No | No |
| Six-gate verification before surfacing tests | Yes | Partial | Partial | Partial | No |
| Visual regression in customer environment | Yes | Vendor-dependent | Vendor-dependent | No | DIY |
| Audit-ready traceability | Yes | Limited | Limited | Limited | No |

### The headline difference

Other tools help you write, run, or manage tests faster. Sentinel helps you close the right gaps in the right order, with the right developer, inside the governance model Stryker already trusts.

That shift from test authoring to coverage management is what makes Sentinel a platform decision rather than a productivity tool.

## 7. Security, Compliance & Deployment

Sentinel is engineered around a single principle: source code and quality evidence should stay under customer control.

The target deployment is single-tenant and aligned with Stryker's cloud, identity, networking, monitoring, and SIEM patterns. The pilot should validate the exact model endpoint, supported region, retention terms, private networking controls, and egress policy before production rollout.

### Security posture at a glance

| Layer | How code stays controlled |
|---|---|
| Network | Runs inside customer-approved Azure networking patterns. Private endpoint and egress controls are validated during pilot setup. |
| Inference | Enterprise-approved model endpoint. Region, retention, and data-use terms reviewed with Stryker security before production use. |
| Identity | Microsoft Entra ID with workload identity federation. No standing keys or shared passwords. |
| Storage | Code indexes, prompts, responses, generated tests, and audit records stored in customer-controlled infrastructure. |
| Audit | Every prompt, response, gap, test, verification result, and notification is recorded for review. |
| Residency | Region and data-flow controls are defined with Stryker security and enforced through Azure policy where applicable. |
| Compliance | Designed to support SOC 2, ISO 27001, HIPAA, GDPR, and regulated engineering traceability workflows. Final control mapping depends on Stryker requirements. |

### Why this matters for the procurement conversation

Many enterprise QA tools are SaaS-first. That can work for some workflows, but it creates friction when source code, regulated product evidence, or sensitive design history are involved. Sentinel is designed to deploy as a governed workload inside Stryker-approved infrastructure, using existing identity, networking, monitoring, and SIEM controls. From the security team's point of view, it should look like one more controlled workload, not a new uncontrolled vendor surface.

## 8. Business Case & Expected Outcomes

Sentinel pays back along three vectors: reclaimed QA capacity, higher coverage faster, and fewer late-stage or escaped defects. The exact numbers vary by team and surface, so the business case should be proven in a focused pilot rather than asserted in advance.

### How we propose to prove the case

We do not want you to take the business case on faith. The four-week pilot is structured around a small set of numbers that Stryker and Sentinel agree on up front. At day 30, the data says yes, no, or tune before expansion.

- **Coverage uplift on the pilot repo.** Measured against the existing baseline before Sentinel was switched on.
- **Number of high-confidence gaps surfaced.** Total volume, broken down by surface and severity.
- **Developer acceptance rate.** Of the gaps surfaced, what fraction were merged unchanged, merged with edits, or dismissed with a reason.
- **Time from gap detection to merged fix.** Median and 90th percentile.
- **QA hours reclaimed.** Self-reported by the pilot QA team, with a baseline captured in week one.
- **Evidence usefulness.** QA, security, and leadership confirm whether the traceability record is useful for governance.

### What we need from Stryker

- A pilot team and repository with active development and an engaged tech lead.
- Read-only access to the chosen repo and the requirement system used by that team.
- A platform/security contact for Azure deployment validation.
- Agreement on the pilot success metrics before kickoff.
- One QA or quality stakeholder to review whether the generated evidence is useful.

## 9. Adoption Roadmap

Sentinel rolls out in three phases. Each phase has explicit entry criteria, exit criteria, and success metrics, so leadership knows at every milestone whether to continue or course-correct.

| Phase | Days 0-30: Pilot | Days 31-60: Expand | Days 61-90: Scale |
|---|---|---|---|
| Scope | One team, one repository. Selected for active development and an engaged tech lead. | Two to four additional teams. Add a second surface, such as mobile or frontend if pilot was backend. | Org-wide rollout across priority teams. Full surface coverage over time: backend, web, mobile. |
| Activities | Deploy into Azure tenant. Index codebase and requirements. Normalize behaviors. First gaps ping developers in week two. | Fold in additional repos. Tune prioritization weights. Roll out Figma awareness and visual regression where applicable. | Standardize across the engineering org. Integrate with internal service catalog and SIEM. Establish ongoing governance cadence. |
| Success metrics | Coverage uplift on pilot repo. Acceptance rate above agreed target. Time-to-merge or dismissal under agreed target. QA hours reclaimed. | Coverage uplift across expanded teams. Acceptance rate stable or improving. First production-defect prevention case study. | Defined org-wide coverage baseline and trajectory. Sentinel owned by an internal champion team. Audit trail in production use. |
| Stryker effort | One tech lead, one QA engineer, one PM, one platform/security contact for setup. | Lower per-team effort as patterns repeat. One coordinator to drive expansion across teams. | Steady-state operation. One owner team for governance and tuning. |

## 10. Risks & Mitigations

A platform decision deserves a clear-eyed view of what could go wrong. The table below captures the concerns we hear most often from engineering and security leadership, and how Sentinel is designed to address each one.

| Risk | How it's mitigated |
|---|---|
| Generated tests are low quality or flaky | The six-gate verifier filters generated tests before any human sees them. Anything that reaches a developer should have compiled, run, passed, stayed stable, added new coverage, and caught an injected bug. |
| Developer fatigue from too many notifications | Hard cap of three to five gaps per developer per week. Everything else stays queued and re-scored. The system optimises for signal, not volume. |
| Source code leaving the intended perimeter | Single-tenant deployment aligned with Stryker-approved Azure patterns. Model endpoint, retention, network flow, and storage controls are validated by security during pilot setup. |
| Vendor lock-in on a single LLM provider | Sentinel's orchestration layer is model-agnostic. The architecture supports swapping providers without rewriting deterministic workers. |
| Inference cost grows unpredictably | Most of Sentinel is deterministic code. AI is reserved for high-leverage workers. Budget caps and per-team quotas are configurable. |
| Sentinel suggests tests for code that is about to be deleted | Dismissals carry a reason, and the system learns from them. It stops surfacing similar gaps in areas teams have flagged as obsolete or low value. |
| Internal team cannot operate or extend the platform | Architecture is documented end to end. Worker services are stateless and replaceable. Knowledge transfer is part of the engagement. |

## 11. Engagement Model & Next Steps

Recommended next step: run a four-week pilot on one active Stryker repository.

At the end of the pilot, Stryker should receive:

- Baseline coverage map.
- Top high-confidence gaps and reason codes.
- Example generated test PRs.
- Six-gate verification reports.
- Owner-routing rationale.
- Accepted, edited, dismissed, and merged log.
- Security and data-flow summary.
- Recommendation: expand, tune, or stop.

## Appendix: Technical Deep-Dive

This appendix captures the technical detail that engineering reviewers will want to see. Leadership readers can safely skip it.

### A.1 Architecture overview

Sentinel runs as seven worker services orchestrated by a coordinator, with three persistent stores: a code index, a requirements store, and an audit log, backed by Postgres and Azure Blob or equivalent customer-approved services. Walking the system top to bottom: a small smart layer on top, seven worker services in the middle, three storage stores at the bottom, and a deliberate gap before the customer's CI because Sentinel never replaces test execution. It hands off to whatever the team already uses.

### A.2 AI usage breakdown

Of the seven worker services, only two are full AI agents, three are pure deterministic code, and two are hybrids.

| Worker | AI usage | What it does |
|---|---|---|
| Ingester | None | Tree-sitter and LSP. Parses syntax, resolves types, builds the dependency graph. Tools beat models at parsing. |
| Prioritiser | None | Weighted scoring math. Risk, churn, blast radius, defects, business priority. Must be explainable to the team. |
| Verifier | None | Six gates run as containerised test commands. Pass/fail is binary. Determinism is non-negotiable. |
| Mapper | Hybrid | Pattern matching first, vector similarity second, LLM reasoning only for ambiguous mappings. |
| Notifier | Hybrid | Routing is deterministic: Git blame, CODEOWNERS, service catalog. Message copy can be generated so it reads human, not robotic. |
| Requirements Parser | Full AI | Reads Jira, Confluence, Figma, Notion, or Azure DevOps. Returns strict JSON. Free-form prose becomes structured behaviours. |
| Generator | Full AI | Writes actual test code. Multiple candidates are generated and filtered before verifier gates. |

### A.3 The six verification gates

| # | Gate | What it checks |
|---|---|---|
| 1 | Builds | Compiles cleanly. Imports resolve. No syntax issues. |
| 2 | Runs | Executes end to end with mocks and fixtures wired up. |
| 3 | Passes today | Green against current code. The test is a baseline, not a bug report. |
| 4 | Stable | Same result on every re-run. Flaky tests do not ship. |
| 5 | Adds coverage | Hits code or behavior your existing tests do not cover. No duplicates. |
| 6 | Catches bugs | Sentinel breaks the code on purpose. The test has to notice. |

### A.4 Frontend coverage strategy

Frontend gaps split into three categories. Behavioural gaps, such as "empty form submit shows an error," are caught with component tests and Playwright. Visual gaps, such as "button should be blue" or "card has 16px padding," are caught with Storybook plus visual comparison in your environment. Experience gaps, such as "loading spinner appears when API is slow," are caught with user-journey scripts and accessibility audits.

### A.5 Mobile coverage strategy

Mobile is roughly twice the work of web because iOS and Android are different worlds. Sentinel uses the native stack on each side: XCTest, XCUITest, and snapshot testing on iOS; JUnit, Robolectric, Espresso, and screenshot testing on Android. For cross-platform, it supports Jest and Detox or Maestro on React Native; flutter_test, integration_test, and golden tests on Flutter. The generator detects the framework from package.json, pubspec.yaml, or native project files.

End of document. Sentinel · v1.1 · May 2026
