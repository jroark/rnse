// CloudFront Function (cloudfront-js-2.0), viewer request, on rns-e.com's
// distribution: the site is folders of index.html in a private S3 bucket,
// which does not resolve directory URLs by itself.
//   /x/  -> /x/index.html
//   /x   -> 301 to /x/   (when the last part has no dot: a page, not a file)
// Deployed as the function "rns-e-index"; this file is its source.
function handler(event) {
  var request = event.request;
  var uri = request.uri;
  if (uri.endsWith("/")) {
    request.uri = uri + "index.html";
    return request;
  }
  var last = uri.substring(uri.lastIndexOf("/") + 1);
  if (last.indexOf(".") === -1) {
    var qs = "";
    var keys = Object.keys(request.querystring);
    if (keys.length) {
      qs = "?" + keys.map(function (k) {
        var v = request.querystring[k];
        return v.value === "" ? k : k + "=" + v.value;
      }).join("&");
    }
    return {
      statusCode: 301,
      statusDescription: "Moved Permanently",
      headers: { location: { value: uri + "/" + qs } },
    };
  }
  return request;
}
