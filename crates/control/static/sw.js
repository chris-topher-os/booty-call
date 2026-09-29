const CACHE = 'oswitch-v1';

self.addEventListener('install', (e) => self.skipWaiting());

self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()));

// Cache-first for the app shell; /api always goes to the network.
self.addEventListener('fetch', (e) => {
  if (e.request.url.includes('/api/')) return;
  e.respondWith(
    caches.match(e.request).then(
      (hit) =>
        hit ||
        fetch(e.request).then((res) => {
          const copy = res.clone();
          caches.open(CACHE).then((c) => c.put(e.request, copy));
          return res;
        })
    )
  );
});
