# Sentinel

## Coverage Intelligence for Regulated Engineering

Sentinel maps requirements, code, tests, risk, and ownership, then turns the highest-confidence coverage gaps into verified test changes and audit-ready evidence.

| Prepared for | Stryker Engineering Leadership |
|---|---|
| Document type | Solution Brief |
| Version | v1.2, May 2026 |
| Audience | Engineering, QA, Security, Compliance, Product Leadership |

## 1. Executive Summary

AI is making code creation faster. It is not automatically making software safer, better tested, or easier to audit.

For Stryker, this matters because software quality is tied to product reliability, regulatory evidence, cybersecurity, and executive quality governance. Sentinel is designed for that environment.

Sentinel continuously reads:

- Code
- Requirements
- Existing tests
- Design intent
- Change history
- Ownership signals

It then identifies the most important coverage gaps, generates tests where appropriate, verifies them before developer review, and records the evidence trail.

The goal is not "more tests." The goal is:

> Know what should be covered, what is actually covered, what risk remains, who owns it, and what evidence proves it.

## 2. Why This Matters Now

Engineering velocity is rising. AI-assisted development is increasing code output. Product surfaces are expanding across backend, web, mobile, APIs, embedded workflows, robotics, and cloud-connected systems.

Traditional QA processes were not built for that pace.

The risk is not only escaped defects. The bigger leadership risk is losing confidence in whether coverage still matches requirements, risk, and product behavior.

Research supports this concern:

- DORA's recent AI research shows AI can improve productivity, but outcomes depend on strong engineering systems.
- FDA's Quality Management System Regulation became effective on February 2, 2026 and incorporates ISO 13485:2016 into 21 CFR Part 820.
- FDA software assurance guidance emphasizes risk-based confidence in automated systems used for production and quality workflows.
- Stryker publicly emphasizes quality data reviewed with executive leadership, broad ISO 13485 certification, and regular independent audits.

That makes Sentinel a leadership tool, not just a developer tool.

## 3. The Problem

Most teams do not lack testing tools. They lack a continuous answer to five questions:

| Leadership question | Why it matters |
|---|---|
| What required behavior is not tested? | Prevents hidden product and quality risk. |
| What code changed without enough coverage? | Reduces release and regression risk. |
| Which gaps matter most? | Protects developer focus. |
| Who owns each gap? | Removes QA/developer coordination drag. |
| What evidence exists? | Supports audit, governance, and release confidence. |

Existing tools help write, run, review, or manage tests. Sentinel connects the full picture.

## 4. What Sentinel Does

Sentinel runs a continuous coverage loop:

| Step | What happens | Evidence produced |
|---|---|---|
| Ingest | Reads code, requirements, design sources, tests, and change history. | Indexed system snapshot. |
| Map | Links requirements, code paths, tests, and owners. | Coverage graph. |
| Prioritize | Scores gaps by risk, churn, blast radius, defects, and business priority. | Explainable risk ranking. |
| Generate | Creates test candidates for high-confidence gaps. | Test PR or patch. |
| Verify | Runs build, execution, stability, coverage, and negative checks. | Verification report. |
| Route | Sends the action to the right owner. | Ticket, PR comment, or Slack/Teams message. |
| Learn | Tracks accepted, edited, dismissed, and merged outcomes. | Audit trail and updated map. |

## 5. Why Sentinel Is Better for Stryker

Sentinel is strongest where Stryker's environment is hardest: regulated software, complex product systems, distributed ownership, and evidence-driven quality.

| Stryker need | Sentinel advantage |
|---|---|
| Quality governance | Shows coverage risk across requirements, code, tests, and ownership. |
| Audit readiness | Produces traceable evidence for each surfaced gap and decision. |
| Developer focus | Sends only high-confidence, risk-ranked gaps to owners. |
| QA leverage | Automates mechanical mapping, checking, routing, and evidence capture. |
| Security posture | Designed for tenant-contained deployment and customer-controlled evidence stores. |
| Tool coexistence | Integrates with existing CI, test management, issue tracking, and notification tools. |

The authentic claim is not that Sentinel replaces every QA tool.

The authentic claim is:

> Sentinel is the coverage intelligence layer above testing tools. It helps leadership see which quality gaps matter, why they matter, who owns them, and what evidence proves action was taken.

## 6. Competitive Landscape

The market is crowded, but most tools solve narrower problems.

| Category | Examples | Strength | Sentinel difference |
|---|---|---|---|
| Low-code / agentic test automation | Testim, mabl, Functionize, Testsigma, Katalon | Faster UI, mobile, API, and E2E test creation. | Sentinel starts from coverage risk, not test authoring. |
| Visual AI testing | Applitools | Visual, accessibility, and cross-device validation. | Sentinel identifies where visual evidence is missing and records the trail. |
| Unit-test generation | Diffblue | Strong Java/Kotlin unit-test automation. | Sentinel is broader across requirements, surfaces, risk, and ownership. |
| AI code review | GitHub Copilot, Qodo, CodeRabbit | PR feedback, standards, suggested fixes. | Sentinel finds gaps beyond the current PR diff. |
| Test management / quality analytics | Tricentis, Parasoft, Azure DevOps Test Plans | Planning, execution tracking, traceability, dashboards. | Sentinel feeds a live coverage graph and action queue into those systems. |

### Headline Difference

Other tools help teams create, run, or review tests faster.

Sentinel helps leadership know whether the right things are covered.

## 7. Security and Deployment

Sentinel is designed for customer-controlled deployment and evidence ownership.

| Layer | Target posture |
|---|---|
| Deployment | Single-tenant, tenant-contained deployment aligned with Stryker-approved Azure patterns. |
| Access | Read-only repository access by default, scoped to pilot repositories. |
| Identity | Microsoft Entra ID and workload identity patterns. |
| Storage | Customer-controlled storage for indexes, prompts, outputs, verification, and audit logs. |
| Model use | Enterprise-approved model endpoint, with region, retention, networking, and data-flow reviewed before production use. |
| Audit | Traceable record of inputs, generated output, verification result, owner routing, and human decision. |

Security claims should be validated during pilot setup. Sentinel should not rely on vague promises like "trust us." The data flow, retention model, egress policy, and evidence store should be reviewable by Stryker security.

## 8. Pilot Business Case

Sentinel should prove value in a focused pilot before expansion.

| Metric | What it proves |
|---|---|
| Accepted high-confidence gaps | Signal quality. |
| Coverage uplift on selected surfaces | Real improvement, not dashboard noise. |
| Requirement-code-test traceability | Audit and governance value. |
| Time from gap to action | Operating speed. |
| QA/developer effort saved | Capacity reclaimed. |
| False-positive rate | Trustworthiness. |

### Pilot Scope

- One active team
- One repository
- One requirement source
- Existing CI integration
- One notification path
- Agreed success and stop criteria

### 30-Day Output

At the end of the pilot, Stryker should receive:

- Baseline coverage map
- Top risk-ranked gaps
- Example generated test PRs
- Verification reports
- Owner-routing rationale
- Accepted / edited / dismissed / merged log
- Security and data-flow summary
- Recommendation: expand, tune, or stop

## 9. Adoption Roadmap

| Phase | Focus | Success signal |
|---|---|---|
| Days 0-30 | Pilot one team and repository. | Useful gaps, accepted PRs, clear evidence trail. |
| Days 31-60 | Expand to two to four teams and another surface. | Stable acceptance rate and broader traceability. |
| Days 61-90 | Scale governance and reporting. | Internal owner team, dashboard, audit-ready workflow. |

## 10. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Low-quality generated tests | Verification gates before developer review. |
| Developer fatigue | Limit surfaced gaps to high-confidence, risk-ranked items. |
| Ambiguous requirements | Flag ambiguity instead of generating false certainty. |
| AI encodes current behavior instead of intended behavior | Require requirement, design, or risk rationale for each generated test. |
| Security concerns | Validate model endpoint, storage, retention, and egress before production use. |
| Tool overlap | Integrate with existing tools; do not force rip-and-replace. |

## 11. Recommended Next Step

Run a 30-day pilot on one active Stryker repository.

Decision at day 30:

- **Expand** if gaps are useful, evidence is trusted, and developer acceptance is strong.
- **Tune** if mapping is useful but generated tests need improvement.
- **Stop** if signal quality, security posture, or operational fit is not proven.

## 12. Research and Source Notes

Key sources supporting the framing:

- Stryker Global Quality: https://www.stryker.com/us/en/about/global-quality.html
- Stryker Mako SmartRobotics: https://www.stryker.com/us/en/joint-replacement/systems/Mako_SmartRobotics_Overview.html
- FDA Quality Management System Regulation: https://www.fda.gov/medical-devices/postmarket-requirements-devices/quality-management-system-regulation-qmsr
- FDA Computer Software Assurance guidance: https://www.fda.gov/regulatory-information/search-fda-guidance-documents/computer-software-assurance-production-and-quality-management-system-software
- FDA Cybersecurity in Medical Devices guidance: https://www.fda.gov/regulatory-information/search-fda-guidance-documents/cybersecurity-medical-devices-quality-management-system-considerations-and-content-premarket
- DORA 2024 report: https://dora.dev/research/2024/dora-report/
- DORA 2025 report: https://dora.dev/research/2025/dora-report/
- Tricentis Testim: https://www.tricentis.com/products/test-automation-web-apps-testim
- mabl: https://www.mabl.com/
- Functionize: https://www.functionize.com/
- Testsigma: https://testsigma.com/
- Katalon: https://katalon.com/
- Applitools: https://applitools.com/
- Diffblue Cover: https://cover-docs.diffblue.com/get-started/what-is-diffblue-cover
- GitHub Copilot test writing: https://docs.github.com/en/copilot/tutorials/write-tests
- Qodo code review: https://docs.qodo.ai/code-review
- CodeRabbit docs: https://docs.coderabbit.ai/
- Parasoft: https://www.parasoft.com/

---

End of document. Sentinel · v1.2 · May 2026
