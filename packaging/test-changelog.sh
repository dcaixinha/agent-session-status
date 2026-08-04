#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
source "$ROOT/packaging/changelog.sh"

temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT
changelog="$temporary/CHANGELOG.md"
cat > "$changelog" <<'EOF'
# Changelog

## [Unreleased]

### Added

- Verified the initial release rollover.

[Unreleased]: https://github.com/dcaixinha/agent-session-status/compare/v0.1.0...HEAD
EOF

cat > "$temporary/invalid.md" <<'EOF'
## [Unreleased]

### Added

### Internal

- This is not a user-facing Keep a Changelog category.
EOF
! changelog_has_unreleased_changes "$temporary/invalid.md"

changelog_has_unreleased_changes "$changelog"
changelog_roll "$changelog" 0.1.0 2026-08-04 '' \
  dcaixinha/agent-session-status "$temporary"
changelog_extract_notes "$changelog" 0.1.0 "$temporary/0.1.0.md"
grep -q '^### Added$' "$temporary/0.1.0.md"
! grep -q '<!--' "$temporary/0.1.0.md"
! grep -q '^\[Unreleased\]:' "$temporary/0.1.0.md"
grep -q '^\[0.1.0\]: https://github.com/dcaixinha/agent-session-status/releases/tag/v0.1.0$' \
  "$changelog"

awk '
  { print }
  /^<!-- Add user-visible changes below using Keep a Changelog categories\. -->$/ {
    print ""
    print "### Fixed"
    print ""
    print "- Verified a second release rollover."
  }
' "$changelog" > "$temporary/next-changelog"
mv "$temporary/next-changelog" "$changelog"
changelog_has_unreleased_changes "$changelog"
changelog_roll "$changelog" 0.2.0 2026-08-05 v0.1.0 \
  dcaixinha/agent-session-status "$temporary"
changelog_extract_notes "$changelog" 0.2.0 "$temporary/0.2.0.md"
grep -q '^- Verified a second release rollover\.$' "$temporary/0.2.0.md"
! grep -q '<!--' "$temporary/0.2.0.md"
grep -q '^\[Unreleased\]: https://github.com/dcaixinha/agent-session-status/compare/v0.2.0\.\.\.HEAD$' \
  "$changelog"
grep -q '^\[0.2.0\]: https://github.com/dcaixinha/agent-session-status/compare/v0.1.0\.\.\.v0.2.0$' \
  "$changelog"
