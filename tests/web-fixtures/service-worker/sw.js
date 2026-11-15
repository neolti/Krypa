// Krypa service-worker fixture. Plain script, no imports.
// Precaches the fixture files, serves probe.txt from cache with an
// X-Krypa-Source header, and falls back to offline.html for navigations
// that fail at the network level.
var CACHE = 'krypa-sw-fixture-v1';
var PRECACHE = ['./', './index.html', './offline.html', './probe.txt'];

self.addEventListener('install', function (event) {
  event.waitUntil(
    caches.open(CACHE)
      .then(function (cache) { return cache.addAll(PRECACHE); })
      .then(function () { return self.skipWaiting(); })
  );
});

self.addEventListener('activate', function (event) {
  event.waitUntil(self.clients.claim());
});

function tagged(response, source) {
  var headers = new Headers(response.headers);
  headers.set('X-Krypa-Source', source);
  return response.blob().then(function (body) {
    return new Response(body, { status: response.status, statusText: response.statusText, headers: headers });
  });
}

self.addEventListener('fetch', function (event) {
  var request = event.request;
  var url = new URL(request.url);
  if (request.method !== 'GET' || url.origin !== self.location.origin) return;

  if (request.mode === 'navigate') {
    event.respondWith(
      fetch(request).catch(function () {
        return caches.match('./offline.html');
      })
    );
    return;
  }

  if (url.pathname.slice(-'/probe.txt'.length) === '/probe.txt') {
    event.respondWith(
      caches.match(request).then(function (cached) {
        if (cached) return tagged(cached, 'cache');
        return fetch(request).then(function (res) { return tagged(res, 'network'); });
      })
    );
  }
});
