"""Names the tests that passed only on a retry in a nextest run.

Reads nextest's JUnit report (target/nextest/ci/junit.xml, the ci profile's)
and, for each test with a flaky failure, prints a GitHub warning and adds a
line to the job's summary. It never fails the job: a test that failed every
attempt has failed the check step already.
"""

import os
import sys
import xml.etree.ElementTree as ET

path = os.path.join("target", "nextest", "ci", "junit.xml")
if not os.path.exists(path):
    print("No nextest report; no tests ran.")
    sys.exit(0)

flaky = []
for case in ET.parse(path).iter("testcase"):
    failures = case.findall("flakyFailure") + case.findall("flakyError")
    if failures:
        flaky.append((case.get("classname", ""), case.get("name", ""), len(failures)))

if not flaky:
    print("No flaky tests.")
    sys.exit(0)

for binary, name, count in flaky:
    print(f"::warning title=Flaky test::{binary} {name} passed only on a retry "
          f"({count} failed attempt(s)). Fix it: a test must pass on its first run.")
summary = os.environ.get("GITHUB_STEP_SUMMARY")
if summary:
    lines = [f"- `{binary}` `{name}`: passed after {count} failed attempt(s)" for binary, name, count in flaky]
    with open(summary, "a", encoding="utf-8") as out:
        out.write("### Flaky tests\n\nThese passed only on a retry, so the run passed. Each needs fixing.\n\n")
        out.write("\n".join(lines) + "\n")
