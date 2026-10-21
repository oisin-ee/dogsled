UNIT dotfiles-skills: new and fixed harness skills (dotfiles repo).
Base: origin/main 6ecc577. Branch: feat/workforce-skills.
Read first: AGENTS.md, conf.d/harness/docs/skills.md, conf.d/harness/docs/rules.md, conf.d/harness/skills/{research,to-tickets,code-standard,design,audit}/SKILL.md and their references to match style and frontmatter (including disable-model-invocation where a method is explicit-only). Read ~/.agents/rules/20-delegation.md and 10-evidence-and-continuity.md: the skills operationalize those rules and must not restate them.
Deliverable:
1. skills/brief-writing/SKILL.md: the dispatch contract a lead writes for a child: deliverable, owned and generated paths, base SHA, acceptance command, allowed light checks, forbidden heavy checks, publication authority, and reply shape. Default-discoverable.
2. skills/integration/SKILL.md: how a lead reconciles child results: verify evidence against the exact revision, merge order, conflict handling, then run the repository gate once at the final revision. Default-discoverable.
3. skills/plan-review/SKILL.md: read-only review of a plan or ticket set for contradictions in dependencies, acceptance criteria, ownership, and unresolved decisions. Default-discoverable.
4. skills/adr/SKILL.md: write a proposed ADR in the repository's existing convention (detect it: infra uses descriptive filenames under docs/adr with status/context/decisions/consequences; rondo uses numbered records with an index README and maintainer approval). Fields follow Nygard plus MADR (drivers, options, consequences, confirmation). Only the user accepts a record. Explicit-only.
5. skills/research/references/compare-libraries.md and a conditional link from research/SKILL.md: eligibility gates first (constraints, licence, platform, maintenance), then trade-offs, rejected options, one recommendation, evidence freshness and decision sensitivity; never propose forks, patches, or upstream work; no invented numeric scores.
6. skills/design/SKILL.md: fix routing so it reaches its existing references (review.md, browser-grounding.md, design-exploration.md, design-token-enforcement.md, visual-diff.md) by task mode (implement, review, explore). Do not rewrite the references.
7. conf.d/harness/docs/skills.md: list the new methods in the right table.
Owned paths: conf.d/harness/skills/{brief-writing,integration,plan-review,adr}/**, conf.d/harness/skills/research/SKILL.md, conf.d/harness/skills/research/references/compare-libraries.md, conf.d/harness/skills/design/SKILL.md, conf.d/harness/docs/skills.md.
Write in the repository's existing plain style (short sentences, no filler).
Acceptance: `for f in conf.d/harness/skills/{brief-writing,integration,plan-review,adr}/SKILL.md; do head -6 "$f" | grep -q '^name: ' && head -6 "$f" | grep -q '^description: ' || echo "BAD $f"; done` prints nothing; every relative link in the changed SKILL.md files resolves to an existing file (check with a short rg+test loop); lefthook pre-commit passes on commit.

Rules for this unit:
- Work only in your worktree. Stay inside the owned paths. Read the repository's AGENTS.md or README and its conventions before you edit.
- Light checks only: the formatter or linter on the files you changed, the type check of the changed package, and the unit tests that cover your change. Do not run full suites, builds of other packages, e2e, dependency installs, or CI watching. The lead runs the repository gate once, after integration.
- Commit hooks run on commit. If a hook fails, fix findings inside your scope; otherwise report blocked with the hook output. Never bypass hooks.
- Commit with feat|fix|chore|docs|test(scope): description, push the branch, and open a DRAFT PR. Do not merge it.
- Never read, print, or commit secrets. Do not fork, patch, or vendor third-party code.
- Final reply: branch, head SHA, PR URL, files changed, the light checks you ran with results, and anything blocked or unverified. Do not spawn agents.
- Plan of record: the agent-workforce plan (sections quoted below are authoritative).