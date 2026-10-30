# Behaviour specs

A behaviour spec writes down how a Beatlayer feature behaves where it has to match Ableton Live 12: what Live does, how we know, and the rules Beatlayer follows as a result. Specs are the bridge between Live and Beatlayer's code in the [clean-room policy](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code). Code is written from a spec, never from Live itself, and every rule in a spec is encoded in a test.

## When you need one

Write a spec before you build a feature whose behaviour has to match Live in detail, such as launch quantisation, follow actions, warp-marker rules, groove timing, or the shortcuts in a view. A work package that reproduces Live behaviour writes or updates its spec first (step 3 of "How an agent uses this plan" in the [build plan](../03-build-plan.md)), so the spec is in the package's pull request alongside the code and tests.

You don't need a spec for anything Beatlayer defines for itself, such as its project file or extension API. [The architecture](../02-architecture.md) covers those.

## Sources you may cite

Cite only the allowed sources in the clean-room policy, and name each one precisely enough that someone else can check it:

- **Manual.** A chapter and section of Ableton's reference manual, or a set of release notes, with the Live version. Paraphrase it; never copy its text into the repository.
- **Observation.** What you saw while running Live: the Live version, the operating system, the steps you took, and what happened. Reading a running Live's UI through the operating system's accessibility interface counts as observation.
- **Measurement.** Live's output from your own input material: the method, the input, the settings, the tool, and the result with its tolerance. Generate the input with a script in `tests/fixtures/gen/` where you can. Record numbers, not audio, because Live's renders stay on your own machine.
- **Product page.** Ableton's public page for a device or pack (its URL and the date read), for devices the reference manual doesn't describe, such as Granulator III and the Creative Extensions pack's Bass and Poli. Paraphrase it.
- **Paper.** A published paper, book or standard reference (authors, title, venue, year) for the physics and signal processing a behaviour rests on, such as the partial ratios of a bar or the bowed-string friction curve; never for what Live does.

## Write a spec

1. Copy [TEMPLATE.md](TEMPLATE.md) to `docs/specs/<topic>.md`, where the topic is short, lower-case, and hyphenated, such as `launch-quantisation.md`.
1. Describe the behaviour in your own words, and list every source in the sources table.
1. Write the rules as numbered statements a test can check, and give each rule the sources it rests on. Where two sources disagree, list both in the sources table and write the rule only for what the sources show.
1. Write a test for each rule, and list it in the tests table.
1. Set the status: `draft` while a rule has no test, `partial` once some rules have passing tests, and `implemented` when every rule has a passing test.

## Rule IDs

Rules are numbered `R1`, `R2`, and so on within each spec, and the spec's file name and the number together identify a rule anywhere in the repository: `launch-quantisation R3`. Mark each test with a comment that names its spec and rule, such as `// Spec: launch-quantisation R3`, so a search finds every test for a rule.

Never renumber rules. To change a rule, edit it and give the reason in the pull request. To withdraw one, strike it through and keep its number.

## When Live changes

When a Live release changes a behaviour that a spec records, add the new source with its version, change the rule, and change its tests, all in one pull request. Live's release notes are the place to look first, and the [parity matrix](../04-parity-matrix.md) lists the releases it covers.
