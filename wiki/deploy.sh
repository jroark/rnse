#!/bin/sh
# Publish target/wiki/site (see wiki/build.sh) to the S3 bucket behind the
# CloudFront distribution and have the distribution fetch it afresh.
#   AWS_PROFILE=... BUCKET=name DISTRIBUTION=id wiki/deploy.sh
# The bucket is made to match the site: files no longer in it are deleted.
# Nothing that looks like firmware, a disc or a card image ever goes up.
# .github/workflows/wiki.yml runs this on pushes to main (no stored keys:
# an IAM role that trusts the repository's main branch; see the workflow).
set -e
cd "$(dirname "$0")/../target/wiki/site"
: "${BUCKET:?name of the bucket}" "${DISTRIBUTION:?id of the CloudFront distribution}"

bad=$(find . -type f ! \( -name '*.html' -o -name '*.css' -o -name '*.js' -o -name '*.wasm' \
      -o -name '*.png' -o -name '*.jpg' -o -name '*.svg' \))
[ -z "$bad" ] || { echo "unexpected files:" >&2; echo "$bad" >&2; exit 1; }
big=$(find . -type f -size +25M)
[ -z "$big" ] || { echo "files over 25 MB:" >&2; echo "$big" >&2; exit 1; }
[ -f index.html ] && [ -f emulator/index.html ] && [ -f emulator/rnse.wasm ] \
    || { echo "incomplete site; run wiki/build.sh" >&2; exit 1; }

s3="s3://$BUCKET"
q="--only-show-errors"
# Pages, styles and scripts change with every build: a short cache time.
aws s3 sync . "$s3" $q --delete --exclude '*' --include '*.html' \
    --content-type 'text/html; charset=utf-8' --cache-control 'max-age=300'
aws s3 sync . "$s3" $q --exclude '*' --include '*.css' \
    --content-type 'text/css; charset=utf-8' --cache-control 'max-age=300'
aws s3 sync . "$s3" $q --exclude '*' --include '*.js' \
    --content-type 'text/javascript; charset=utf-8' --cache-control 'max-age=300'
aws s3 sync . "$s3" $q --exclude '*' --include '*.svg' \
    --content-type 'image/svg+xml' --cache-control 'max-age=86400'
aws s3 sync . "$s3" $q --exclude '*' --include '*.wasm' \
    --content-type application/wasm --cache-control 'max-age=300'
# Pictures rarely change.
aws s3 sync . "$s3" $q --exclude '*' --include '*.png' \
    --content-type image/png --cache-control 'max-age=86400'
aws s3 sync . "$s3" $q --exclude '*' --include '*.jpg' \
    --content-type image/jpeg --cache-control 'max-age=86400'
# Last, remove what the site no longer has (the first sync only deleted
# HTML files, since its filters exclude everything else).
aws s3 sync . "$s3" $q --delete --exclude '*.html'
aws cloudfront create-invalidation --distribution-id "$DISTRIBUTION" --paths '/*' \
    --query Invalidation.Id --output text
