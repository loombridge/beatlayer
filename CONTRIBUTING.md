# Contributing to Beatlayer

Thank you for helping build Beatlayer. This page explains how a change gets from your machine into `main`: what to read first, how work is organised, the checks every change passes, and the certificate you give with each commit.

Beatlayer is in Phase 0 of its [build plan](docs/03-build-plan.md), and the repository stays private until the public alpha. Everyone who takes part follows the [code of conduct](CODE_OF_CONDUCT.md).

## Read first

1. [README.md](README.md) and [AGENTS.md](AGENTS.md): what Beatlayer is, the lines no change crosses, and where things stand.
1. [The clean-room policy](https://github.com/loombridge/beatlayer/blob/main/CONTRIBUTING.md#building-without-abletons-code): what you may and may not learn from Ableton Live. Read it before you write anything.
1. [The architecture](docs/02-architecture.md), at least the sections your change touches.
1. [The build plan](docs/03-build-plan.md), and in it "How an agent uses this plan". That procedure is how every work package is done, by a person or an agent.

## The clean room

Beatlayer matches what Live does, not how Live's code does it. In short:

- Learn only from what Live shows any user: the manual, running Live and watching it, and measuring its output from your own material.
- Never copy Ableton's samples, presets, artwork, or manual text into the repository, and never use Ableton's trademarks as names.

## Find something to work on

- **Work packages.** Most work is a package in the build plan. Take the first package in the status table whose status is `todo` and whose dependencies are `done`, or the one the maintainer gives you. Each package has its own branch, `wp/<id>-<slug>` (for example `wp/0.12-contributor-docs`), and its own pull request.
- **Bugs and features.** Open an issue first, with the bug report or feature request template. A feature request names the row of the [parity matrix](docs/04-parity-matrix.md) it relates to.
- **Small fixes**, such as a typo or a broken link, can go straight to a pull request.

## Make the change

1. If your change reproduces Live behaviour, write its behaviour spec first. [docs/specs/README.md](docs/specs/README.md) explains how, and [the template](docs/specs/TEMPLATE.md) gives the shape.
1. Build to the acceptance criteria. Each criterion is a test or a command whose output you paste into the pull request.
1. Give every feature a headless test path, through `bl-cli` or a unit test. A feature that can only be tested by clicking isn't done.
1. Keep to the rules in [AGENTS.md](AGENTS.md): no allocation, lock, or blocking call on an audio thread; engine, model, and device crates never depend on a UI crate; no dependency that `cargo deny` rejects; no plug-in binaries, Ableton files, or third-party audio in the repository.
1. Run every check. It must pass before you open the pull request.

   ```sh
   cargo xtask check
   ```

1. If a golden test fails, your change altered what the engine renders. If that's a mistake, fix it. If it's intended, remake the references and say in the pull request why each one changed:

   ```sh
   cargo xtask golden --update
   ```

## Commit

- Stage files by path. Never use `git add -A` or `git add .`, and check `git status` so build output, `.DS_Store`, and scratch files stay out.
- Start the subject line with the package ID when there is one, for example `WP-0.12: contributor docs`.
- Sign off every commit, as the next section explains.

## Developer Certificate of Origin

Beatlayer takes contributions under the [Developer Certificate of Origin](https://developercertificate.org) (DCO), version 1.1, instead of a contributor licence agreement. You keep the copyright in your work. When you sign off a commit, you certify the statements below: in short, that you wrote the change, or otherwise have the right to submit it under the project's open-source licence, and that the record of your contribution is public.

```text
Developer Certificate of Origin
Version 1.1

Copyright (C) 2004, 2006 The Linux Foundation and its contributors.

Everyone is permitted to copy and distribute verbatim copies of this
license document, but changing it is not allowed.


Developer's Certificate of Origin 1.1

By making a contribution to this project, I certify that:

(a) The contribution was created in whole or in part by me and I
    have the right to submit it under the open source license
    indicated in the file; or

(b) The contribution is based upon previous work that, to the best
    of my knowledge, is covered under an appropriate open source
    license and I have the right under that license to submit that
    work with modifications, whether created in whole or in part
    by me, under the same open source license (unless I am
    permitted to submit under a different license), as indicated
    in the file; or

(c) The contribution was provided directly to me by some other
    person who certified (a), (b) or (c) and I have not modified
    it.

(d) I understand and agree that this project and the contribution
    are public and that a record of the contribution (including all
    personal information I submit with it, including my sign-off) is
    maintained indefinitely and may be redistributed consistent with
    this project or the open source license(s) involved.
```

### How to sign off

A sign-off is a `Signed-off-by` line at the end of the commit message, with your name and email address. Git adds it from your `user.name` and `user.email` settings when you commit with `-s`:

```sh
git commit -s -m "WP-0.12: contributor docs"
```

The message then ends with a line like this one:

```text
Signed-off-by: Your Name <you@example.com>
```

Use the name you're known by in the project and an email address that reaches you. Anonymous sign-offs aren't accepted.

If you forget:

- For the last commit, run `git commit --amend --no-edit --signoff`.
- For every commit on your branch, run `git rebase --signoff origin/main`, then `git push --force-with-lease`.

To see which commits on your branch are signed off, run:

```sh
git log origin/main..HEAD --format='%h %s%n  %(trailers:key=Signed-off-by,valueonly,separator=%x2C )'
```

A commit with nothing on the indented line under it isn't signed off.

### Changes written with an AI agent

If an AI coding agent wrote some or all of your change, you're still the person certifying it. Review the change, then sign it off yourself. The agent records its part with a `Co-Authored-By` trailer, and never adds a `Signed-off-by` line, in your name or its own, because only a person can give the certificate.

The maintainer's own work is the exception. Commits that the maintainer's agents make on the maintainer's behalf, in work packages from the build plan, are the maintainer's contributions and carry no sign-off (10 October 2026). The DCO check applies to everyone else.

### Checking sign-offs

For now, the reviewer checks sign-offs by hand. An automatic check in CI follows the CI work in WP-0.2.

## Open a pull request

- Open it against `main`. For a work package, title it `WP-<id>: <package title>`.
- Fill in every section of the template: the work package, what the change adds, the acceptance evidence, any deviation from the design with its reason, anything that needs the maintainer, and the clean-room declaration.
- Update any document whose facts your change alters, in the same pull request.
- The maintainer reviews and merges. Don't merge your own pull request.

## Report a bug or ask for a feature

Use the issue templates. A bug report asks for the steps, your platform, and your audio setup. A feature request asks which row of the parity matrix it relates to. Describe Live's behaviour in your own words; don't paste Ableton's text, and don't attach Ableton's samples, presets, or audio rendered from them.

## Write in the house style

Write prose, including documents, code comments, commit messages, and pull requests, in plain British English, with sentence-case headings. Address the reader in the second person. `rustfmt` and `clippy` settle the code's formatting, and `cargo xtask check` runs both.

## Licence

Beatlayer is licensed under GPL-3.0-or-later ([LICENSE](LICENSE)), and you contribute under the same licence. The maintainer confirmed the licence and the DCO on 10 October 2026 (decision D3 in [the architecture](docs/02-architecture.md#decisions-log), human task H2 in the build plan).
