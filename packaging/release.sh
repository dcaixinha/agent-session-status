#!/usr/bin/env bash
set -euo pipefail

DRY_RUN=false
WAIT_FOR_AUR=true
VERSION=

usage() {
  cat <<'EOF'
Usage: packaging/release.sh [--dry-run] [--no-wait] <version>

Create or resume a stable GitHub release and automatic AUR publication.
The version must be a bare X.Y.Z value without a leading v.

  --dry-run  Validate all publication prerequisites and show the actions.
  --no-wait  Do not wait for the Publish AUR workflow to finish.
EOF
}

while (($#)); do
  case $1 in
    --dry-run|-n) DRY_RUN=true ;;
    --no-wait) WAIT_FOR_AUR=false ;;
    --help|-h)
      usage
      exit 0
      ;;
    -*)
      printf 'error: unknown option: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
    *)
      if [[ -n $VERSION ]]; then
        printf 'error: unexpected argument: %s\n' "$1" >&2
        exit 2
      fi
      VERSION=$1
      ;;
  esac
  shift
done

STABLE_VERSION_REGEX='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
if [[ -z $VERSION || ! $VERSION =~ $STABLE_VERSION_REGEX ]]; then
  echo 'error: version must be a bare stable X.Y.Z value' >&2
  usage >&2
  exit 2
fi

TAG="v$VERSION"
REPOSITORY=dcaixinha/agent-session-status
ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || {
  echo 'error: run this script from the agent-session-status repository' >&2
  exit 1
}
cd "$ROOT"
source "$ROOT/packaging/changelog.sh"

say() {
  printf '\033[1;34m==>\033[0m %s\n' "$*"
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

run_release_gates() {
  local version=$1
  local temporary=$2

  cargo fmt --check
  cargo clippy --locked --all-targets --all-features -- -D warnings
  cargo test --locked --all-features
  cargo build --locked --release --all-features
  go run github.com/rhysd/actionlint/cmd/actionlint@v1.7.7 \
    .github/workflows/ci.yml .github/workflows/aur.yml
  packaging/test-changelog.sh

  sed \
    -e "s/@PKGVER@/$version/g" \
    -e 's/@PKGREL@/1/g' \
    -e 's/@SHA256@/0000000000000000000000000000000000000000000000000000000000000000/g' \
    packaging/aur/PKGBUILD.template > "$temporary/PKGBUILD"
  (
    cd "$temporary"
    makepkg --printsrcinfo > .SRCINFO
  )
  grep -q '^pkgbase = agent-session-status$' "$temporary/.SRCINFO"
  grep -q "^[[:space:]]*pkgver = $version$" "$temporary/.SRCINFO"
  grep -q '^[[:space:]]*pkgrel = 1$' "$temporary/.SRCINFO"
  ! grep -Eq '@(PKGVER|PKGREL|SHA256)@|SKIP' \
    "$temporary/PKGBUILD" "$temporary/.SRCINFO"
}

for command in git cargo gh go makepkg jq; do
  require_command "$command"
done

say 'Checking repository and release prerequisites'
[[ $(git branch --show-current) == main ]] || die 'releases must be created from main'
[[ -z $(git status --porcelain) ]] || die 'working tree is not clean'
gh auth status >/dev/null 2>&1 || die 'gh is not authenticated'

is_canonical_remote() {
  case $1 in
    git@github.com:dcaixinha/agent-session-status.git | \
      ssh://git@github.com/dcaixinha/agent-session-status.git | \
      https://github.com/dcaixinha/agent-session-status.git) return 0 ;;
    *) return 1 ;;
  esac
}

origin_url=$(git remote get-url origin)
is_canonical_remote "$origin_url" \
  || die "origin fetch URL is not canonical: $origin_url"
mapfile -t origin_push_urls < <(git remote get-url --push --all origin)
((${#origin_push_urls[@]} > 0)) || die 'origin has no push URL'
for origin_push_url in "${origin_push_urls[@]}"; do
  is_canonical_remote "$origin_push_url" \
    || die "origin push URL is not canonical: $origin_push_url"
done

visibility=$(gh repo view "$REPOSITORY" --json visibility --jq .visibility)
[[ $visibility == PUBLIC ]] || die 'repository must be public before releasing'
gh secret list --repo "$REPOSITORY" --env aur --json name --jq '.[].name' \
  | grep -qx AUR_SSH_PRIVATE_KEY \
  || die 'configure AUR_SSH_PRIVATE_KEY in the GitHub aur environment first'

git fetch --quiet origin main --tags
behind=$(git rev-list --count HEAD..origin/main)
ahead=$(git rev-list --count origin/main..HEAD)
[[ $behind == 0 ]] || die 'local main is behind origin/main'

current_version=$(awk -F'"' '/^version = "/{print $2; exit}' Cargo.toml)
[[ $current_version =~ $STABLE_VERSION_REGEX ]] \
  || die 'could not read the current Cargo package version'

remote_tag_commit=$(git ls-remote origin "refs/tags/$TAG^{}" | awk '{print $1}')
if [[ -z $remote_tag_commit ]]; then
  remote_tag_commit=$(git ls-remote origin "refs/tags/$TAG" | awk '{print $1}')
fi
local_tag_commit=
if git rev-parse --verify --quiet "refs/tags/$TAG" >/dev/null; then
  local_tag_commit=$(git rev-list -n1 "$TAG")
fi

release_exists=false
resume_release_commit=false
if release_json=$(gh release view "$TAG" --repo "$REPOSITORY" \
  --json isDraft,isPrerelease,tagName 2>/dev/null); then
  jq -e --arg tag "$TAG" \
    '.tagName == $tag and .isDraft == false and .isPrerelease == false' \
    <<<"$release_json" >/dev/null \
    || die "existing release is not a published stable release: $TAG"
  release_exists=true
fi

if [[ -n $remote_tag_commit ]]; then
  [[ $ahead == 0 ]] || die 'main must be synchronized after the release tag was pushed'
  [[ $current_version == "$VERSION" ]] || die 'Cargo version does not match existing tag'
  if $release_exists; then
    git merge-base --is-ancestor "$remote_tag_commit" HEAD \
      || die 'existing release tag is not an ancestor of current main'
    tagged_version=$(git show "$TAG:Cargo.toml" \
      | awk -F'"' '/^version = "/{print $2; exit}')
    [[ $tagged_version == "$VERSION" ]] || die 'tagged Cargo version does not match release'
  else
    [[ $remote_tag_commit == "$(git rev-parse HEAD)" ]] \
      || die 'unpublished release tag does not point at current main'
  fi
elif [[ -n $local_tag_commit ]]; then
  [[ $current_version == "$VERSION" ]] || die 'Cargo version does not match local tag'
  [[ $local_tag_commit == "$(git rev-parse HEAD)" ]] \
    || die 'local release tag does not point at HEAD'
  if [[ $ahead != 0 ]]; then
    [[ $ahead == 1 && $(git log -1 --format=%s) == "🔖 Release $TAG" ]] \
      || die 'unexpected unpushed commits exist with the local release tag'
  fi
else
  if [[ $ahead != 0 ]]; then
    [[ $ahead == 1 && $current_version == "$VERSION" \
      && $(git log -1 --format=%s) == "🔖 Release $TAG" ]] \
      || die 'main must be synchronized with origin/main'
    resume_release_commit=true
  fi
fi

$release_exists && [[ -n $remote_tag_commit ]] \
  || ! $release_exists \
  || die 'GitHub release exists without a remote release tag'

if [[ -z $local_tag_commit && -z $remote_tag_commit && $VERSION != "$current_version" ]]; then
  newest=$(printf '%s\n%s\n' "$current_version" "$VERSION" | sort -V | tail -n1)
  [[ $newest == "$VERSION" ]] || die "$VERSION is older than current version $current_version"
fi

[[ -f CHANGELOG.md ]] || die 'CHANGELOG.md is missing'
new_release=false
if [[ -z $local_tag_commit && -z $remote_tag_commit ]] && ! $resume_release_commit; then
  new_release=true
  grep -q '^## \[Unreleased\]$' CHANGELOG.md \
    || die 'CHANGELOG.md has no [Unreleased] section'
  grep -q '^\[Unreleased\]:' CHANGELOG.md \
    || die 'CHANGELOG.md has no [Unreleased] comparison link'
  changelog_has_unreleased_changes CHANGELOG.md \
    || die 'CHANGELOG.md [Unreleased] needs a change bullet under a standard category'
else
  grep -q "^## \[$VERSION\] - [0-9]\{4\}-[0-9]\{2\}-[0-9]\{2\}$" CHANGELOG.md \
    || die "CHANGELOG.md has no dated $VERSION release section"
fi

if $DRY_RUN; then
  say 'Running non-mutating release gates'
  dry_temporary=$(mktemp -d)
  (
    trap 'rm -rf "$dry_temporary"' EXIT
    run_release_gates "$VERSION" "$dry_temporary"
  )

  say "Dry run for $TAG"
  if $release_exists; then
    echo "Would re-dispatch AUR publication for existing release $TAG."
  elif [[ -n $remote_tag_commit ]]; then
    echo "Would publish the missing GitHub release for remote tag $TAG."
  elif [[ -n $local_tag_commit ]]; then
    echo "Would push the existing local tag $TAG and publish its GitHub release."
  elif $resume_release_commit; then
    echo "Would resume the existing release commit by creating and pushing tag $TAG."
    echo "Would publish GitHub release $TAG with curated changelog notes."
  else
    if [[ $VERSION == "$current_version" ]]; then
      echo "Would roll CHANGELOG.md and create commit: 🔖 Release $TAG"
    else
      echo "Would update Cargo.toml and Cargo.lock from $current_version to $VERSION."
      echo "Would roll CHANGELOG.md and create commit: 🔖 Release $TAG"
    fi
    echo "Would create annotated tag $TAG at the resulting release commit."
    echo "Would atomically push main and $TAG."
    echo "Would publish GitHub release $TAG with curated changelog notes."
  fi
  echo "Would run all Rust, workflow, and PKGBUILD metadata checks."
  echo "Would $($WAIT_FOR_AUR && echo wait for || echo not wait for) AUR publication."
  exit 0
fi

temporary=$(mktemp -d)
restore_release_files=false
cleanup() {
  status=$?
  trap - EXIT
  if ((status != 0)) && $restore_release_files; then
    git restore --staged CHANGELOG.md Cargo.toml Cargo.lock >/dev/null 2>&1 || true
    cp "$temporary/CHANGELOG.md" CHANGELOG.md
    if [[ -f $temporary/Cargo.toml ]]; then
      cp "$temporary/Cargo.toml" Cargo.toml
      cp "$temporary/Cargo.lock" Cargo.lock
    fi
  fi
  rm -rf "$temporary"
  exit "$status"
}
trap cleanup EXIT

if $new_release; then
  cp CHANGELOG.md "$temporary/CHANGELOG.md"
  restore_release_files=true

  if [[ $VERSION != "$current_version" ]]; then
    say "Updating package version to $VERSION"
    cp Cargo.toml Cargo.lock "$temporary/"
    sed -i "0,/^version = \"$current_version\"$/s//version = \"$VERSION\"/" Cargo.toml
    cargo check --quiet
    [[ $(awk -F'"' '/^version = "/{print $2; exit}' Cargo.toml) == "$VERSION" ]]
    lock_version=$(awk '
      /^name = "agent-session-status"$/ { found = 1; next }
      found && /^version = / { gsub(/version = |"/, ""); print; exit }
    ' Cargo.lock)
    [[ $lock_version == "$VERSION" ]] || die 'Cargo.lock version did not update'
  fi

  previous_tag=$(gh release list --repo "$REPOSITORY" --limit 100 \
    --json tagName,isDraft,isPrerelease \
    --jq '[.[] | select(.isDraft == false and .isPrerelease == false)][0].tagName // empty')
  say "Rolling CHANGELOG.md into $VERSION"
  changelog_roll CHANGELOG.md "$VERSION" "$(date +%F)" "$previous_tag" \
    "$REPOSITORY" "$temporary"
fi

changelog_extract_notes CHANGELOG.md "$VERSION" "$temporary/release-notes.md" \
  || die "CHANGELOG.md $VERSION release notes are empty"

say 'Running release gates'
run_release_gates "$VERSION" "$temporary"

if $new_release; then
  unexpected=$(git status --porcelain \
    | grep -Ev '^ M CHANGELOG\.md$|^ M Cargo\.toml$|^ M Cargo\.lock$' || true)
  [[ -z $unexpected ]] || die "unexpected files changed during release:\n$unexpected"
  git add CHANGELOG.md Cargo.toml Cargo.lock
  git commit -m "🔖 Release $TAG"
  restore_release_files=false
fi

if [[ -z $local_tag_commit && -z $remote_tag_commit ]]; then
  say "Creating annotated tag $TAG"
  git tag -a "$TAG" -m "agent-session-status $TAG"
  local_tag_commit=$(git rev-list -n1 "$TAG")
fi

if [[ -z $remote_tag_commit ]]; then
  say 'Atomically pushing main and release tag'
  git push --atomic origin main "$TAG"
  remote_tag_commit=$local_tag_commit
fi

workflow_event=release
workflow_commit=$remote_tag_commit
if $release_exists; then
  workflow_event=workflow_dispatch
  workflow_commit=$(git rev-parse HEAD)
  previous_run_id=$(gh run list --repo "$REPOSITORY" --workflow aur.yml \
    --event "$workflow_event" --commit "$workflow_commit" --limit 1 \
    --json databaseId --jq '.[0].databaseId // empty')
  say 'Re-dispatching AUR publication for existing release'
  gh workflow run aur.yml --repo "$REPOSITORY" --ref main \
    --field tag="$TAG" --field pkgrel=1
else
  previous_run_id=$(gh run list --repo "$REPOSITORY" --workflow aur.yml \
    --event "$workflow_event" --commit "$workflow_commit" --limit 1 \
    --json databaseId --jq '.[0].databaseId // empty')
  say 'Publishing GitHub release'
  gh release create "$TAG" --repo "$REPOSITORY" --verify-tag \
    --title "$TAG" --notes-file "$temporary/release-notes.md"
fi

if ! $WAIT_FOR_AUR; then
  say 'GitHub release is published; AUR publication is running asynchronously'
  exit 0
fi

say 'Waiting for Publish AUR workflow'
run_id=
for _ in {1..30}; do
  candidate=$(gh run list --repo "$REPOSITORY" --workflow aur.yml \
    --event "$workflow_event" --commit "$workflow_commit" --limit 1 \
    --json databaseId --jq '.[0].databaseId // empty')
  if [[ -n $candidate && $candidate != "$previous_run_id" ]]; then
    run_id=$candidate
    break
  fi
  sleep 2
done
[[ -n $run_id ]] || die 'Publish AUR workflow did not start within 60 seconds'
gh run watch "$run_id" --repo "$REPOSITORY" --exit-status

say "Release complete: https://github.com/$REPOSITORY/releases/tag/$TAG"
say 'AUR package: https://aur.archlinux.org/packages/agent-session-status'
