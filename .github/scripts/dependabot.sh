#!/usr/bin/env bash
set -euo pipefail

repository=rkstraessler/ReikemMailClient_RS
required_check='Required CI'

github_api() {
    local path=$1
    shift
    gh api "repos/$repository/$path" "$@"
}

is_open_pull() {
    jq -e --arg repo "$repository" '.state == "open" and .base.repo.full_name == $repo' >/dev/null
}

is_dependabot() {
    jq -e --arg repo "$repository" '
        .user.login == "dependabot[bot]" and .user.id == 49699333 and .user.type == "Bot"
        and .state == "open" and .draft == false and .base.ref == "main"
        and .base.repo.full_name == $repo and .head.repo.full_name == $repo
        and (.head.ref | startswith("dependabot/"))' >/dev/null
}

can_update_branch() {
    jq -e --arg repo "$repository" '
        .state == "open" and .base.repo.full_name == $repo and .mergeable == true
        and .head.repo != null and (.head.repo.full_name != $repo or .head.ref != "main")
        and (.head.repo.full_name == $repo or .maintainer_can_modify == true)' >/dev/null
}

# Classify only signed Dependabot commits and verified GitHub merges made by the installed app.
# Input is a stream containing the pull request, its pages of commits, and its pages of changed files.
classify_dependabot() {
    jq -cs --arg app "$1" '
        def trim: sub("^\\s+"; "") | sub("\\s+$"; "");
        def unquote: trim | sub("^[\"\u0027]"; "") | sub("[\"\u0027]$"; "");
        def bot: .login == "dependabot[bot]" and .id == 49699333 and .type == "Bot";
        def version:
            [capture("^v?(?<major>0|[1-9][0-9]{0,14})\\.(?<minor>0|[1-9][0-9]{0,14})\\.(?<patch>0|[1-9][0-9]{0,14})$")
            | [.major, .minor, .patch] | map(tonumber)][0];
        def classify_version($entry; $declared):
            ($entry.old | version) as $old | ($entry.new | version) as $new
            | if $old == null or $new == null or $old >= $new then {kind:null, safe:false}
              else (if $old[0] != $new[0] then "major"
                    elif $old[1] != $new[1] then "minor" else "patch" end) as $kind
              | {kind:$kind, safe: (($kind == "patch" or ($kind == "minor" and $old[0] > 0))
                                  and $declared == "version-update:semver-\($kind)")}
              end;
        def metadata_records:
            reduce (split("\n")[] | trim) as $line ([];
                if $line | startswith("- dependency-name:") then
                    . + [{name: ($line | ltrimstr("- dependency-name:") | unquote), versions:[], kinds:[]}]
                elif length > 0 and ($line | startswith("dependency-version:")) then
                    .[-1].versions += [($line | ltrimstr("dependency-version:") | unquote)]
                elif length > 0 and ($line | startswith("update-type:")) then
                    .[-1].kinds += [($line | ltrimstr("update-type:") | unquote)]
                else . end);
        def updates:
            gsub("\r\n"; "\n") | split("\n---\n") as $blocks
            | (($blocks[1] // "") + "\n" | split("\n...\n")) as $trailer
            | if ($blocks | length) != 2 or ($trailer | length) != 2
                 or ($trailer[0] | startswith("updated-dependencies:\n") | not)
                 or ($trailer[1] | trim | test("^(|Signed-off-by: dependabot\\[bot\\] <support@github\\.com>)$") | not)
                 then null
              else ($trailer[0] | metadata_records) as $records
              | [$blocks[0] | split("\n")[]
                 | capture("^Updates `(?<name>[^`\\s]+)` from `?(?<old>[^`\\s]+)`? to `?(?<new>[^`\\s]+)`?\\s*$")] as $grouped
              | (if ($grouped | length) > 0 then $grouped
                 elif ($records | length) == 1 then
                     [$blocks[0] | split("\n")[]
                      | capture("^Bumps .+ from `?(?<old>[^`\\s]+)`? to `?(?<new>[^`\\s]+)`?\\.[ \\t]*$")
                      | .name = $records[0].name]
                 else [] end) as $pairs
              | if ($pairs | length) == 0 or ($pairs | length) != ($records | length)
                   or any(range($pairs | length); . as $i | $pairs[$i].name != $records[$i].name
                          or $records[$i].versions != [$pairs[$i].new] or ($records[$i].kinds | length) != 1)
                then null
                else [range($pairs | length) as $i | classify_version($pairs[$i]; $records[$i].kinds[0])] end
              end;
        def verified_commit:
            . as $entry | .commit as $commit
            | (.committer.login == "web-flow" and .committer.id == 19864447
               and $commit.committer.email == "noreply@github.com") as $web_flow
            | if $commit.verification.verified != true or $commit.verification.reason != "valid" then null
              elif (.parents | length) == 2 and $app != "" and .author.login == "\($app)[bot]"
                   and .author.type == "Bot" and .author.id > 0
                   and ($web_flow or (.author.id == .committer.id and .committer.type == "Bot")) then []
              elif (.parents | length) == 1 and (.author | bot)
                   and $commit.author.email == "49699333+dependabot[bot]@users.noreply.github.com"
                   and ($web_flow or (($entry.committer | bot)
                         and $commit.committer.email == "49699333+dependabot[bot]@users.noreply.github.com"))
                   then $commit.message | updates
              else null end;
        def ecosystem:
            if . == "Cargo.toml" or . == "Cargo.lock" then "rust"
            elif test("^\\.github/workflows/([A-Za-z0-9_-]+\\.ya?ml|actions\\.lock)$") then "github-actions"
            else null end;
        .[0] as $pr | (.[1] | add // []) as $commits | (.[2] | add // []) as $files
        | ([$commits[] | verified_commit]) as $commits_verified
        | ([$commits_verified[] | select(. != null) | .[]]) as $updates
        | ([$files[].filename | ecosystem | select(. != null)] + [$updates[].kind | select(. != null)] + ["dependencies"]
            + (if $pr.head.ref | test("^dependabot/(cargo|github_actions)/(rust|actions)-security(-|/|$)") then ["security"] else [] end)
            + (if any($commits[]; .author.login == "autofix-ci[bot]") then ["autofix"] else [] end)) as $labels
        | (($commits | length) == $pr.commits and ($commits | length) > 0
            and ($files | length) == $pr.changed_files and ($files | length) > 0
            and ([$commits[].sha] | unique | length) == ($commits | length)
            and any($commits[]; .sha == $pr.head.sha) and all($commits_verified[]; . != null)
            and all($files[]; (.filename | ecosystem) != null and .status == "modified" and (has("previous_filename") | not))
            and ($updates | length) > 0) as $verified
        | ($verified and all($updates[]; .safe)
            and ($pr.title | test("^\\w+(\\([^\\n]*\\))?!:") | not)
            and all($pr.labels[]; .name != "breaking-change")) as $eligible
        | {eligible:$eligible, verified:$verified,
           labels: (($labels + [if $eligible then "automerge" else "manual-review" end]) | unique)}'
}

label_pull() {
    local number=$1 pull=$2 wanted=$3 current add remove
    current=$(jq -c '[.labels[].name]' <<< "$pull") || return
    add=$(jq -r --argjson old "$current" '.labels - $old | join(",")' <<< "$wanted") || return
    remove=$(jq -r --argjson old "$current" '
        $old - .labels | map(select(. == "patch" or . == "minor" or . == "major"
                      or . == "automerge" or . == "manual-review")) | join(",")' <<< "$wanted") || return
    local arguments=()
    if [[ -n "$add" ]]; then arguments+=(--add-label "$add"); fi
    if [[ -n "$remove" ]]; then arguments+=(--remove-label "$remove"); fi
    if (( ${#arguments[@]} > 0 )); then
        gh pr edit "$number" --repo "$repository" "${arguments[@]}" >/dev/null || return
    fi
}

branch_sha() {
    local encoded
    encoded=$(jq -nr --arg branch "$1" '$branch | split("/") | map(@uri) | join("/")') || return
    github_api "git/ref/heads/$encoded" --jq '.object.sha'
}

ci_passed() {
    local head=$1 ref=$2 run_id
    run_id=$(github_api "actions/workflows/ci.yml/runs?event=pull_request&head_sha=$head&per_page=1" \
        | jq -er --arg sha "$head" --arg ref "$ref" --arg repo "$repository" '
            .workflow_runs[0] | select(.event == "pull_request" and .path == ".github/workflows/ci.yml"
                and .head_sha == $sha and .head_branch == $ref and .head_repository.full_name == $repo
                and .status == "completed" and .conclusion == "success") | .id') || return
    github_api "actions/runs/$run_id/jobs?filter=latest&per_page=100" \
        | jq -e --arg check "$required_check" '[.jobs[] | select(.name == $check)]
            | length == 1 and .[0].conclusion == "success"' >/dev/null
}

protection_active() {
    github_api 'rules/branches/main' | jq -e --arg check "$required_check" '
        any(.[]; .type == "required_status_checks" and .parameters.strict_required_status_checks_policy
            and any(.parameters.required_status_checks[]; .context == $check and .integration_id == 15368))
        and any(.[]; .type == "pull_request" and .parameters.required_approving_review_count >= 1
            and .parameters.dismiss_stale_reviews_on_push)' >/dev/null
}

maintain() {
    local number=$1 pull current head head_ref base_ref base commits files policy eligible=false update=false
    local behind state latest review auto_merge merge_token
    pull=$(github_api "pulls/$number") || return
    if ! is_open_pull <<< "$pull"; then return 0; fi
    head=$(jq -er '.head.sha' <<< "$pull") || return
    head_ref=$(jq -er '.head.ref' <<< "$pull") || return
    base_ref=$(jq -er '.base.ref' <<< "$pull") || return
    if is_dependabot <<< "$pull"; then
        commits=$(github_api "pulls/$number/commits?per_page=100" --paginate --slurp) || return
        files=$(github_api "pulls/$number/files?per_page=100" --paginate --slurp) || return
        policy=$(printf '%s\n' "$pull" "$commits" "$files" | classify_dependabot "${APP_SLUG:-}") || return
        eligible=$(jq -r '.eligible' <<< "$policy") || return
        current=$(github_api "pulls/$number") || return
        if ! is_dependabot <<< "$current" || ! jq -e --arg head "$head" '.head.sha == $head' <<< "$current" >/dev/null; then
            return 0
        fi
        label_pull "$number" "$current" "$policy" || return
        pull=$current
    fi
    if [[ -n "${APP_TOKEN:-}" ]] && can_update_branch <<< "$pull"; then update=true; fi
    if [[ "$eligible" == false && "$update" == false ]] || ! jq -e '.mergeable == true' <<< "$pull" >/dev/null; then
        return 0
    fi
    base=$(branch_sha "$base_ref") || return
    behind=$(github_api "compare/$base...$head?per_page=1" --jq '.behind_by') || return
    if [[ ! "$behind" =~ ^[0-9]+$ ]]; then return 1; fi
    if (( behind > 0 )); then
        if [[ "$update" == true ]]; then
            GH_TOKEN="$APP_TOKEN" github_api "pulls/$number/update-branch" --method PUT \
                -f expected_head_sha="$head" --silent || return
            printf '#%s: requested GitHub branch update\n' "$number"
        else
            printf '#%s: waiting for Dependabot to rebase or for the maintenance app\n' "$number"
        fi
        return 0
    fi
    if [[ "$eligible" == false ]] || ! ci_passed "$head" "$head_ref" || ! protection_active; then return 0; fi
    state=$(gh pr view "$number" --repo "$repository" \
        --json state,isDraft,headRefOid,baseRefName,mergeable,mergeStateStatus,reviewDecision,autoMergeRequest) || return
    if ! jq -e --arg head "$head" '
        .state == "OPEN" and .isDraft == false and .headRefOid == $head and .baseRefName == "main"
        and .mergeable == "MERGEABLE" and (.mergeStateStatus | IN("BEHIND", "DIRTY", "UNKNOWN") | not)
        and .reviewDecision != "CHANGES_REQUESTED"' <<< "$state" >/dev/null; then return 0; fi
    latest=$(github_api "pulls/$number") || return
    if ! is_dependabot <<< "$latest" || ! jq -e --arg head "$head" '.head.sha == $head' <<< "$latest" >/dev/null; then
        return 0
    fi
    if [[ "$(branch_sha main)" != "$base" ]]; then return 0; fi
    if ! printf '%s\n' "$latest" "$commits" "$files" | classify_dependabot "${APP_SLUG:-}" | jq -e '.eligible' >/dev/null; then
        return 0
    fi
    review=$(jq -r '.reviewDecision' <<< "$state") || return
    auto_merge=$(jq -r '.autoMergeRequest != null' <<< "$state") || return
    merge_token=${APP_TOKEN:-${GH_TOKEN:?}}
    if [[ "$review" != APPROVED && -n "${APP_TOKEN:-}" ]]; then
        GH_TOKEN="$APP_TOKEN" github_api "pulls/$number/reviews" --method POST -f commit_id="$head" -f event=APPROVE --silent || return
    fi
    if [[ "$auto_merge" == false ]]; then
        GH_TOKEN="$merge_token" gh pr merge "$number" --repo "$repository" --squash --auto --match-head-commit "$head" || return
        printf '#%s: requested native squash auto-merge\n' "$number"
    fi
}

main() {
    if [[ "${GITHUB_REPOSITORY:-}" != "$repository" ]]; then
        printf 'Unexpected GitHub repository\n' >&2
        return 1
    fi
    if [[ -n "${APP_TOKEN:-}" && ! "${APP_SLUG:-}" =~ ^[a-z0-9-]+$ ]]; then
        printf 'Invalid maintenance app identity\n' >&2
        return 1
    fi
    local numbers number failed=0
    numbers=$(github_api 'pulls?state=open&per_page=100' --paginate --slurp | jq -r 'add // [] | .[].number') || return
    while IFS= read -r number; do
        if [[ ! "$number" =~ ^[0-9]+$ ]]; then continue; fi
        if ! maintain "$number"; then
            printf '#%s: GitHub declined a request or the automation could not verify it\n' "$number" >&2
            failed=1
        fi
    done <<< "$numbers"
    return "$failed"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then main; fi
