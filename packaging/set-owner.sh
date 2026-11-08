#!/bin/sh
# Replace the GitHub owner everywhere (URLs, app ID). Usage: packaging/set-owner.sh NEW_OWNER
set -eu
NEW="${1:?usage: $0 NEW_OWNER}"
cd "$(dirname "$0")/.."
. packaging/meta.env
OLD="$OWNER"
[ "$OLD" = "$NEW" ] && { echo "owner is already $NEW"; exit 0; }
OLD_LC="$(echo "$OLD" | tr 'A-Z' 'a-z')"
NEW_LC="$(echo "$NEW" | tr 'A-Z' 'a-z')"
# Exact-case owner in GitHub URLs, lowercase owner in Pages hosts and app IDs.
git ls-files | grep -vE '^(research/|Cargo.lock$)' | xargs grep -il "$OLD" 2>/dev/null | while read -r f; do
    sed -i -e "s/$OLD/$NEW/g" -e "s/$OLD_LC/$NEW_LC/g" "$f"
    echo "updated $f"
done
echo "Done. Review with: git diff"
