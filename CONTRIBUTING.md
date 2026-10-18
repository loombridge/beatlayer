# Contributing to Beatlayer

Thank you for helping build Beatlayer. This page explains how a change gets from your machine into `main`: what to read first, how work is organised, the checks every change passes, and the certificate you give with each commit.

Beatlayer is in early development, and the repository stays private until the public alpha. Everyone who takes part follows the [code of conduct](CODE_OF_CONDUCT.md).

## Read first

1. [README.md](README.md) and [AGENTS.md](AGENTS.md): what Beatlayer is, the lines no change crosses, and how work is done.
1. [Building without Ableton's code](#building-without-abletons-code): what you may and may not learn from Ableton Live. Read it before you write anything.

## Building without Ableton's code

Beatlayer matches what Ableton Live does, not how Live's code does it. Every contribution, from a person or an agent, keeps to these rules:

- **Learn only from what Live shows any user:** its public reference manual, release notes and product pages; running Live and watching what it does; measuring its output from your own material; and projects you made yourself in Live, for the importer. Paraphrase the manual; never copy its text.
- **Never copy Ableton's content:** no samples, presets, wavetables, impulse responses, grooves, packs, artwork, icons, fonts, colour themes or manual text, in the repository or in Beatlayer's bundled content, and no audio Live rendered from Ableton's own content.
- **Never use Ableton's trademarks as Beatlayer's own name or brand.** Devices, modulators and features take the same functional names as Live's, such as Gate, Saturator or Beat Repeat, so a Live user finds them where they expect. "Imports Ableton Live sets" is a statement of fact; "Ableton Live clone" isn't.
- **Cite your sources.** A change that reproduces Live's behaviour says in its pull request what it rests on: a manual section, an observation of Live running (with its version and platform), or a measurement with its method.

Placement, menus, gestures and keyboard shortcuts match Live's, which is functional parity; the look is Beatlayer's own.

## Find something to work on

- **Work packages.** Most work is a package from the build plan, which a maintainer gives you. Each package has its own branch, `wp/<id>-<slug>`, and its own pull request.
- **Bugs and features.** Open an issue first, with the bug report or feature request template. A feature request says which Live feature it relates to.
- **Small fixes**, such as a typo or a broken link, can go straight to a pull request.

## Make the change

1. If your change reproduces Live behaviour, write its behaviour spec first: what Live does, the source you learned it from (a manual section, an observation of Live running with its version and platform, or a measurement with its method), and the rules Beatlayer follows as a result.
1. Build to the acceptance criteria. Each criterion is a test or a command whose output you paste into the pull request.
1. Give every feature a headless test path, through `bl-cli` or a unit test. A feature that can only be tested by clicking isn't done.
1. Keep to the rules in [AGENTS.md](AGENTS.md): no allocation, lock, or blocking call on an audio thread; engine, model, and device crates never depend on a UI crate; no dependency that `cargo deny` rejects; no plug-in binaries, Ableton files, or third-party audio in the repository.
1. Run the checks. `--affected` tests only the crates your branch changes, committed or not, and the crates that depend on them, and runs every other check (format, clippy, layering, the dependency policy) in full. It must pass before you open the pull request.

   ```sh
   cargo xtask check --affected
   ```

   A change to something every crate shares, such as `Cargo.lock`, xtask, or the shared test fixtures, still tests everything, and so does `cargo xtask check` without `--affected`. Pushes to `main` and the nightly run always test everything.

   The tests run under cargo-nextest, in parallel. The property, stress, and kill tests run fewer cases by default (`BL_TEST_DEPTH=quick`); `BL_TEST_DEPTH=full cargo xtask check` runs them as `main` does.

   Check Windows clippy as well (`cargo xtask clippy-windows`, or `CARGO_FEATURE_PURE=1 cargo clippy --workspace --all-targets --all-features --locked --target x86_64-pc-windows-msvc -- -D warnings`).

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

Commits made by the project itself, authored as Loombridge, carry no sign-off.

### Checking sign-offs

For now, the reviewer checks sign-offs by hand. An automatic check in CI follows the CI work in WP-0.2.

## Open a pull request

- Open it against `main`. For a work package, title it `WP-<id>: <package title>`.
- Fill in every section of the template: the work package, what the change adds, the acceptance evidence, any deviation from the design with its reason, anything that needs a maintainer, and the clean-room declaration.
- Update any document whose facts your change alters, in the same pull request.
- A maintainer reviews and merges. Don't merge your own pull request.

## Report a bug or ask for a feature

Use the issue templates. A bug report asks for the steps, your platform, and your audio setup. A feature request asks which Live feature it relates to. Describe Live's behaviour in your own words; don't paste Ableton's text, and don't attach Ableton's samples, presets, or audio rendered from them.

## Write in the house style

Write prose, including documents, code comments, commit messages, and pull requests, in plain British English, with sentence-case headings. Address the reader in the second person. `rustfmt` and `clippy` settle the code's formatting, and `cargo xtask check` runs both.

## Licence

Beatlayer is licensed under GPL-3.0-or-later ([LICENSE](LICENSE)), and you contribute under the same licence, certifying each commit with the Developer Certificate of Origin.
