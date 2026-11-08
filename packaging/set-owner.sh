#!/bin/sh
# Replace the GitHub owner everywhere (URLs, app ID). Usage: packaging/set-owner.sh NEW_OWNER
set -eu
NEW="${1:?usage: $0 NEW_OWNER}"
cd "$(dirname "$0")/.."
. packaging/meta.env
OLD="$OWNER"
[ "$OLD" = "$NEW" ] && { echo "owner is already $NEW"; exit 0; }
git ls-files | grep -vE '^(research/|Cargo.lock$)' | xargs grep -l "$OLD" 2>/dev/null | while read -r f; do
    sed -i "s/$OLD/$NEW/g" "$f"
    echo "updated $f"
done
echo "Done. Review with: git diff"
