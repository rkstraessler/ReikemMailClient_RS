#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/dependabot.sh"

pull=$(jq -cn --arg repo "$repository" '{
    user: {login:"dependabot[bot]", id:49699333, type:"Bot"}, state:"open", draft:false,
    base:{ref:"main", repo:{full_name:$repo}}, head:{sha:"head", ref:"dependabot/cargo/example", repo:{full_name:$repo}},
    title:"build(deps): bump example", labels:[], changed_files:1, commits:1, mergeable:true}')
files='[[{"filename":"Cargo.lock","status":"modified"}]]'

bot_commit() {
    local message
    message=$(cat <<EOF
Bumps [example](https://example.com/changes) from $1 to $2.

---
updated-dependencies:
- dependency-name: example
  dependency-version: $2
  update-type: version-update:semver-$3
...

Signed-off-by: dependabot[bot] <support@github.com>
EOF
)
    jq -cn --arg message "$message" '{
        sha:"head", parents:[{sha:"old-base"}],
        author:{login:"dependabot[bot]", id:49699333, type:"Bot"}, committer:{login:"web-flow",id:19864447},
        commit:{author:{email:"49699333+dependabot[bot]@users.noreply.github.com"},
                committer:{email:"noreply@github.com"}, verification:{verified:true,reason:"valid"}, message:$message}}'
}

policy() {
    printf '%s\n' "$1" "$2" "$3" | classify_dependabot "${4:-}"
}

test_version() {
    local old=$1 new=$2 kind=$3 eligible=$4 result commit
    commit=$(bot_commit "$old" "$new" "$kind")
    result=$(policy "$pull" "[[$commit]]" "$files")
    jq -e --arg kind "$kind" --argjson eligible "$eligible" '
        .verified and .eligible == $eligible and (.labels | index($kind)) != null
        and (.labels | index(if $eligible then "automerge" else "manual-review" end)) != null' <<< "$result" >/dev/null
}

test_version 1.3.1 1.3.2 patch true
test_version 1.3.1 1.4.0 minor true
test_version 1.3.1 2.0.0 major false
test_version 0.3.1 0.4.0 minor false

base_commit=$(bot_commit 1.3.1 1.3.2 patch)
grouped=$(cat <<'EOF'
Bump compatible dependencies

Updates `alpha` from 1.1.0 to 1.1.2
Updates `beta` from 2.0.0 to 2.1.0

---
updated-dependencies:
- dependency-name: alpha
  dependency-version: 1.1.2
  update-type: version-update:semver-patch
- dependency-name: beta
  dependency-version: 2.1.0
  update-type: version-update:semver-minor
...
EOF
)
grouped_commit=$(jq -c --arg message "$grouped" '.commit.message = $message' <<< "$base_commit")
policy "$pull" "[[$grouped_commit]]" "$files" | jq -e '.eligible and (.labels | index("minor")) != null' >/dev/null
bad_group=$(jq -c '.commit.message |= sub("dependency-version: 2.1.0"; "dependency-version: 3.1.0")' <<< "$grouped_commit")
policy "$pull" "[[$bad_group]]" "$files" | jq -e '.eligible == false' >/dev/null

is_dependabot <<< "$pull"
human=$(jq -c '.user.id = 1 | .head.ref = "feature/example" | .base.ref = "integration"' <<< "$pull")
if is_dependabot <<< "$human"; then exit 1; fi
can_update_branch <<< "$human"
main_head=$(jq -c '.head.ref = "main"' <<< "$human")
if can_update_branch <<< "$main_head"; then exit 1; fi
conflict=$(jq -c '.mergeable = false' <<< "$human")
if can_update_branch <<< "$conflict"; then exit 1; fi
fork=$(jq -c '.head.repo.full_name = "contributor/fork" | .maintainer_can_modify = false' <<< "$human")
if can_update_branch <<< "$fork"; then exit 1; fi
editable_fork=$(jq -c '.maintainer_can_modify = true' <<< "$fork")
can_update_branch <<< "$editable_fork"
spoofed=$(jq -c '.author.id = 1' <<< "$base_commit")
policy "$pull" "[[$spoofed]]" "$files" | jq -e '.eligible == false' >/dev/null
policy "$pull" "[[$base_commit]]" '[[{"filename":"src/client.rs","status":"modified"}]]' \
    | jq -e '.eligible == false' >/dev/null

two_commits=$(jq -c '.commits = 2' <<< "$pull")
initial=$(jq -c '.sha = "previous"' <<< "$base_commit")
merge=$(jq -c '.author = {login:"pr-maintainer[bot]",type:"Bot",id:123} | .parents = [{sha:"previous"},{sha:"updated-main"}]' <<< "$base_commit")
policy "$two_commits" "[[$initial,$merge]]" "$files" pr-maintainer | jq -e '.eligible' >/dev/null
policy "$two_commits" "[[$initial,$merge]]" "$files" other-app | jq -e '.eligible == false' >/dev/null
unsigned_merge=$(jq -c '.commit.verification.verified = false' <<< "$merge")
policy "$two_commits" "[[$initial,$unsigned_merge]]" "$files" pr-maintainer | jq -e '.eligible == false' >/dev/null

printf 'Dependabot version, signature, path, and branch update policy checks passed.\n'
