// Shared by the site's pages (index, programming/, skipper/, security/):
// the sun/moon theme switch and the menu. The theme itself is set before
// paint by a one-line script in each page's head.
(function () {
  var h = document.documentElement;
  var b = document.querySelector('header.top .theme');
  function show() {
    var light = h.dataset.theme === 'light';
    if (!b) { return; }
    b.setAttribute('aria-pressed', light ? 'true' : 'false');
    b.setAttribute('aria-label', light ? 'Switch to dark' : 'Switch to light');
    b.title = b.getAttribute('aria-label');
  }
  if (b) {
    b.addEventListener('click', function () {
      var light = h.dataset.theme === 'light';
      if (light) { delete h.dataset.theme; } else { h.dataset.theme = 'light'; }
      try { localStorage.setItem('helmsman-theme', light ? 'dark' : 'light'); } catch (e) {}
      show();
    });
    show();
  }
  var menu = document.querySelector('details.menu');
  if (menu) {
    document.addEventListener('click', function (e) {
      if (menu.open && !menu.contains(e.target)) { menu.open = false; }
    });
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && menu.open) { menu.open = false; menu.querySelector('summary').focus(); }
    });
  }
})();
