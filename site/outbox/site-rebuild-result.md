## Result: Rebuilt Text Runtime research site

status: done

## Changes made

All 7 pages written to `workspace/text-object-model/site/`:

| File | Lines | Purpose |
|---|---|---|
| `index.html` | 401 | Landing — status dashboard, agent-is-user reframe, built-vs-next table, navigation cards |
| `engine.html` | 451 | The working Rust engine — module breakdown, deps, test status (141/142), format testing, two-store layout, what ISN'T built |
| `architecture.html` | 417 | Settled design — 6-stage pipeline SVG diagram, full data model tables, re-ingestion, re-anchoring cascade, §N agent interface, query/projection contracts |
| `agent-experience.html` | 383 | Agent-as-user perspective — core reframe, three missing layers (Discovery, Read Brief, Output Declaration), TROVE types, Wire provenance, UUID migration problem, built-vs-missing summary |
| `research.html` | 365 | Research landscape — converging pattern (the 5 principles), prior art timeline (1960–2026), 6 key findings, 13 key sources table |
| `decisions.html` | 241 | 8 architectural decisions with chosen/rejected/sources/risk/trade-off for each |
| `roadmap.html` | 271 | Build plan in 5 phases (Pi SDK → Discovery → Read Brief → Output Declaration → UUID protocol), 5 open design questions |

Old pages (`annotations.html`, `hard-problems.html`, `stable-identity.html`, `synthesis.html`, `the-pattern.html`, `transclusion.html`) remain on disk but are not linked from the new site. They serve as archive.

## What shifted from the old site

- **Old framing**: Research-phase, 6 dimensions, human-centric
- **New framing**: Engine is built. Agent is the user. Here's what's done, here's what's next.
- **Old pages**: the-pattern, stable-identity, annotations, transclusion, architecture, hard-problems, synthesis
- **New pages**: engine, architecture, agent-experience, research, decisions, roadmap

## Design

- Same CSS foundation as old site (Inter font, card grid, color-coded sections) — kept the DNA
- Color palette shifted: green (built), slate (architecture), amber (agent experience/design gaps), ink (research), rose (decisions), crimson (roadmap)
- New elements: status dashboard table (index), pipeline SVG diagram (architecture), built-vs-missing table (agent-experience), prior art timeline (research), phase cards with status badges (roadmap)
- No external dependencies — pure HTML/CSS (same as old site)

## How to verify

1. Open `site/index.html` in a browser — check the status dashboard, reframe box, navigation cards
2. Navigate to each sub-page via the top nav — verify all 7 pages link to each other
3. Check the pipeline SVG renders correctly on `architecture.html`
4. Read `agent-experience.html` — this is the most important new page. Verify the three layer cards, TROVE grid, and built-vs-missing table are clear
5. All pages have matching `<html>`, `</html>`, `<body>`, `</body>`, `<head>`, `</head>` counts (verified)

## Notes

- The old sub-pages are still on disk but orphaned from navigation. They can be deleted or moved to an archive directory if desired.
- The pipeline SVG is inline — no external image dependency.
- All pages are responsive (flex/grid layout with media queries).
