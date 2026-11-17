// Report dashboards (rainmill.reporting): packs the widgets of a dashboard so
// one sits directly under the widget above it in its columns, whatever the
// heights beside it. Each widget keeps the columns the dashboard gave it.
(function () {
  var UNIT = 8;

  function pack(grid) {
    if (getComputedStyle(grid).getPropertyValue("--rp-packed").trim() !== "1") {
      grid.querySelectorAll(".rp-widget").forEach(function (w) { w.style.gridRowEnd = ""; });
      return;
    }
    var gap = parseFloat(getComputedStyle(grid).columnGap) || 16;
    grid.querySelectorAll(".rp-widget").forEach(function (w) {
      var body = w.firstElementChild ? w : null;
      if (!body) return;
      w.style.gridRowEnd = "";
      var height = w.getBoundingClientRect().height;
      w.style.gridRowEnd = "span " + Math.ceil((height + gap) / UNIT);
    });
  }

  function watch(grid) {
    if (grid.dataset.rpWatched) return;
    grid.dataset.rpWatched = "1";
    var pending = 0;
    var again = function () {
      cancelAnimationFrame(pending);
      pending = requestAnimationFrame(function () { pack(grid); });
    };
    if (window.ResizeObserver) {
      var seen = new ResizeObserver(again);
      seen.observe(grid);
      grid.querySelectorAll(".rp-widget > *").forEach(function (el) { seen.observe(el); });
      grid.addEventListener("htmx:afterSwap", function (e) {
        var widget = e.target.closest && e.target.closest(".rp-widget");
        if (widget) widget.querySelectorAll(":scope > *").forEach(function (el) { seen.observe(el); });
        again();
      });
    }
    grid.addEventListener("htmx:afterSettle", again);
    window.addEventListener("resize", again);
    again();
  }

  function scan() { document.querySelectorAll(".rp-grid").forEach(watch); }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", scan);
  else scan();
  document.addEventListener("htmx:load", scan);
})();
