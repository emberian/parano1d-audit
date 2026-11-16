# Untracked-file review

Reviewed 2026-11-15. This is a repository housekeeping note, not a test result. The local probe runs described below were not performed; an automatic cybersecurity review stopped a related dynamic test.

| Path | Recommendation | Reason |
| --- | --- | --- |
| `whitepaper/.render-check/` | Keep locally; ignore in Git. | Generated page images and contact sheets for layout review. The `.gitignore` rule leaves the files on disk. |
| `snapshot-release/README.md` | Commit if the planned experiment should remain documented. | It states the fixture, bounds, and observations needed, while making no claim that the run occurred. Review its status when the experiment is resolved. |
| `snapshot-release/middle_header.py` | Leave local for now; do not ignore. | It is an unrun dynamic-test runner. Source belongs in Git only after the stopped test has been reviewed and its publication is intentional. |
| `new-avenues/Cargo.toml`, `Cargo.lock`, `src/main.rs` | Leave local for now; do not ignore. | These are source and dependency resolution for an interrupted, unrun network probe. The committed lane report already records that limit. Commit them together only after reviewing the stopped probe; they are not build artifacts. |

The existing `target/`, `__pycache__/`, `run/`, and `bin/` rules cover build and execution output in these lanes. No untracked source or report is hidden by the new rule.
