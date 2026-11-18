#!/usr/bin/env bash
set -euo pipefail

repo=rkstraessler/ReikemMailClient_RS
while IFS='|' read -r name color description; do
    gh label create "$name" --repo "$repo" --color "$color" --description "$description" --force
done <<'LABELS'
dependencies|41627D|Dependency updates
rust|376B86|Rust crates and tooling
github-actions|376B86|GitHub Actions workflows and dependencies
patch|4E7659|SemVer patch update
minor|587550|SemVer minor update
major|8D5C49|SemVer major update
security|9A5259|Security fix or advisory
automerge|47736E|Eligible for automated merge after CI
manual-review|876D45|Maintainer review required
autofix|655888|Automatic code formatting and lint fixes
bug|955654|Something is broken
enhancement|4F6B91|New or improved behavior
documentation|606E83|Documentation changes
maintenance|69727A|Project maintenance
LABELS
