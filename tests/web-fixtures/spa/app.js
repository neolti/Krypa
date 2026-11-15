// Krypa SPA fixture router. Plain script, no dependencies.
// Routes are matched on location.pathname with a trailing slash enforced.
// Deep links work on reload because each route has its own index.html
// (see spa/about/index.html and spa/items/42/index.html), so a plain
// static server such as `python -m http.server` can serve them.
(function () {
  var views = {
    '/spa/': { name: 'home', text: 'Home view. Use the links to push routes into history.' },
    '/spa/about/': { name: 'about', text: 'About view. Reached with history.pushState or a reload of /spa/about/.' },
    '/spa/items/42/': { name: 'item-42', text: 'Item 42 view. Deep link target: reloading /spa/items/42/ must show this view.' }
  };

  function normalise(path) {
    return path.charAt(path.length - 1) === '/' ? path : path + '/';
  }

  function render() {
    var path = normalise(location.pathname);
    var view = views[path] || { name: 'not-found', text: 'Unknown route. Expected for this fixture: no such route exists.' };
    document.getElementById('current-route').textContent = location.pathname;
    document.getElementById('current-view').textContent = view.name;
    document.getElementById('view-text').textContent = view.text;
    document.getElementById('route-count').textContent = String(history.length);
  }

  function go(path) {
    history.pushState({ path: path }, '', path);
    render();
  }

  document.addEventListener('click', function (e) {
    var link = e.target.closest ? e.target.closest('a[data-spa]') : null;
    if (!link) return;
    e.preventDefault();
    go(link.getAttribute('href'));
  });

  var unknown = document.getElementById('link-unknown');
  if (unknown) {
    unknown.addEventListener('click', function (e) {
      e.preventDefault();
      go('/spa/no-such-route/');
    });
  }

  window.addEventListener('popstate', render);
  render();
})();
