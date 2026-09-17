# Step 7 — Review, then harden

Read `README.md` first. Steps 1–6 must be done.

This follows the repo's per-issue commit chain: implement → review → harden.

## 7a. Code review

- Run the `/code-review` skill on the range from `31c6c35` (last #82 commit)
  to `HEAD`, against issue #83 and spec #70 §D-9/§D-10.
- Check each item in `README.md` "What #83 asks for" has a test that proves
  it. Write the mapping (criterion → test name) in the commit body.

- Run the full verification.


## 7b. Harden

Look for the edge cases a first pass misses. Add tests (and code only if a
test fails):

- a run whose **every** row is reversed: summary has zero rows, total N$0.00,
  excluded count = all rows; register "still live" total is zero;
- a replacement that is itself reversed later: register of the correction
  run shows it `Reversed`; summary excludes it;
- a Person name corrected after finalization: register, summary and batch
  payslips all still show the frozen name;
- a run with a leaver (prorated) and a member with overtime and medical aid:
  register figures equal `get_finalized_payroll_detail` figures per row;
- concurrent reversal during a read does not produce a register whose two
  totals disagree with its own rows (one read transaction — assert rows sum
  to totals in every test via a shared helper);
- very long names and 50 rows in all three PDFs: no overlap
  (`text_placements`).

Run the full verification and the e2e suite.

Commit: `Harden the run outputs of #83`.

Tick step 7 in `progress.txt`. Then close the loop: comment on #83 with the
three commit hashes only when the user asks.
