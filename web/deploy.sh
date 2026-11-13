#!/bin/sh
# Upload target/web/site (see build.sh) to the S3 bucket behind a
# CloudFront distribution and have the distribution fetch it afresh.
#   BUCKET=name DISTRIBUTION=id web/deploy.sh
# Only the four files of the site are sent.  No firmware, ever.
set -e
cd "$(dirname "$0")/../target/web/site"
: "${BUCKET:?name of the bucket}" "${DISTRIBUTION:?id of the CloudFront distribution}"
[ "$(find . -type f | wc -l | tr -d ' ')" = 4 ] || { echo "unexpected files in $(pwd)" >&2; exit 1; }
aws s3 cp index.html "s3://$BUCKET/index.html" --content-type 'text/html; charset=utf-8' --cache-control 'max-age=300' --only-show-errors
aws s3 cp worker.js "s3://$BUCKET/worker.js" --content-type 'text/javascript; charset=utf-8' --cache-control 'max-age=300' --only-show-errors
aws s3 cp rnse.wasm "s3://$BUCKET/rnse.wasm" --content-type application/wasm --cache-control 'max-age=300' --only-show-errors
aws s3 cp faceplate.png "s3://$BUCKET/faceplate.png" --content-type image/png --cache-control 'max-age=86400' --only-show-errors
aws cloudfront create-invalidation --distribution-id "$DISTRIBUTION" --paths '/*' --query Invalidation.Id --output text
