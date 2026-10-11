<!-- Read CONTRIBUTING.md first. Replace each comment with your answer and keep every heading. -->

## Work package

<!-- The package ID and title from docs/03-build-plan.md, such as "WP-0.12 Contributor docs and the clean-room gate", or "None" and the issue this closes. -->

## What it adds

<!-- A few sentences or a short list. -->

## Acceptance evidence

<!-- Each acceptance criterion, with the test or command that shows it and the output. Include the summary from `cargo xtask check`. -->

## Deviations

<!-- Each deviation from the architecture or the build plan, with its reason. Write "None" if there are none. -->

## Needs the maintainer

<!-- Anything you couldn't do here: a decision, hardware, a listening check. Write "Nothing" if there's nothing. -->

## Clean-room declaration

- [ ] I followed the [clean-room rules](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code). This change contains no Ableton code or content, and doesn't use Ableton's trademarks as names.
- [ ] Any Live behaviour this change reproduces is written down in a behaviour spec in `docs/specs/` that cites only allowed sources, or the change reproduces none.

## Sign-off

- [ ] Every commit is signed off under the [Developer Certificate of Origin](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#developer-certificate-of-origin) (`git commit -s`).
