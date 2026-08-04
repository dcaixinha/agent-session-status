#!/usr/bin/env bash

changelog_has_unreleased_changes() {
  local changelog=$1
  awk '
    /^## \[Unreleased\]$/ { in_section = 1; next }
    in_section && /^## \[/ { exit }
    in_section && /^### / {
      category = ($0 ~ /^### (Added|Changed|Deprecated|Removed|Fixed|Security)$/)
      next
    }
    in_section && category && /^- / { found = 1 }
    END { exit(found ? 0 : 1) }
  ' "$changelog"
}

changelog_extract_notes() {
  local changelog=$1
  local version=$2
  local output=$3
  awk -v heading="## [${version}] - " '
    index($0, heading) == 1 { in_section = 1; next }
    in_section && /^## \[/ { exit }
    in_section && /^\[[^]]+\]: / { exit }
    in_section && /^<!-- .* -->$/ { next }
    in_section { lines[++count] = $0 }
    END {
      first = 1
      while (first <= count && lines[first] == "") first++
      last = count
      while (last >= first && lines[last] == "") last--
      for (i = first; i <= last; i++) print lines[i]
    }
  ' "$changelog" > "$output"
  [[ -s $output ]]
}

changelog_roll() {
  local changelog=$1
  local version=$2
  local date=$3
  local previous_tag=$4
  local repository=$5
  local temporary=$6

  awk -v version="$version" -v date="$date" '
    /^## \[Unreleased\]$/ {
      in_unreleased = 1
      print
      print ""
      print "<!-- Add user-visible changes below using Keep a Changelog categories. -->"
      print ""
      print "## [" version "] - " date
      next
    }
    in_unreleased && /^<!-- Add user-visible changes below using Keep a Changelog categories\. -->$/ {
      next
    }
    in_unreleased && /^## \[/ { in_unreleased = 0 }
    { print }
  ' "$changelog" > "$temporary/changelog-sections"

  local release_link
  if [[ -n $previous_tag ]]; then
    release_link="https://github.com/$repository/compare/${previous_tag}...v${version}"
  else
    release_link="https://github.com/$repository/releases/tag/v${version}"
  fi
  awk -v version="$version" -v release_link="$release_link" -v repository="$repository" '
    /^\[Unreleased\]:/ {
      print "[Unreleased]: https://github.com/" repository "/compare/v" version "...HEAD"
      print "[" version "]: " release_link
      next
    }
    { print }
  ' "$temporary/changelog-sections" > "$changelog"
}
