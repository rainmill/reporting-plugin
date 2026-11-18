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

  // Arranging a dashboard. The model is the rows of widgets; every change
  // rewrites each widget's columns and order, and the grid packs them.
  var COLUMNS = 20, MIN = 3;

  function edit(bar) {
    var grid = bar.parentElement.querySelector(".rp-grid");
    if (!grid || bar.dataset.rpBound) return;
    bar.dataset.rpBound = "1";
    var rows = [], hidden = [], dragged = null, before = null;

    function read() {
      rows = []; var byRow = {};
      grid.querySelectorAll(".rp-widget").forEach(function (el) {
        var key = el.dataset.rpRow;
        if (!byRow[key]) { byRow[key] = []; rows.push(byRow[key]); }
        byRow[key].push({ code: el.dataset.rpCode, width: +el.dataset.rpWidth, el: el });
      });
    }
    function draw() {
      rows = rows.filter(function (r) { return r.length; });
      rows.forEach(function (row, i) {
        var start = 1;
        row.forEach(function (w) {
          w.el.dataset.rpRow = i + 1; w.el.dataset.rpWidth = w.width;
          w.el.style.setProperty("--span", w.width);
          w.el.style.setProperty("--start", start);
          start += w.width;
          grid.appendChild(w.el);
        });
      });
      if (strip) grid.appendChild(strip);
      pack(grid);
    }
    function find(code) {
      for (var r = 0; r < rows.length; r++)
        for (var i = 0; i < rows[r].length; i++)
          if (rows[r][i].code === code) return { row: r, index: i, item: rows[r][i] };
      return null;
    }
    function used(row, except) {
      return row.reduce(function (n, w) { return n + (w === except ? 0 : w.width); }, 0);
    }
    function copy() { return rows.map(function (r) { return r.slice(); }); }
    function refuse(el) {
      el.classList.remove("rp-nofit"); void el.offsetWidth; el.classList.add("rp-nofit");
    }

    var strip = document.createElement("div");
    strip.className = "rp-newrow";
    strip.textContent = bar.dataset.rpNewRow || "Drop here for a new row";

    function tools(el) {
      if (el.querySelector(".rp-tools")) return;
      var box = document.createElement("div");
      box.className = "rp-tools";
      var remove = document.createElement("button");
      remove.type = "button"; remove.className = "rp-tools__btn"; remove.title = "Remove";
      remove.setAttribute("aria-label", "Remove"); remove.textContent = "\u00d7";
      remove.addEventListener("click", function () {
        var at = find(el.dataset.rpCode); if (!at) return;
        rows[at.row].splice(at.index, 1); hidden.push(at.item.code);
        el.remove(); draw();
      });
      box.appendChild(remove); el.appendChild(box);
      if (!el.classList.contains("rp-widget--bare")) {
        var grip = document.createElement("div");
        grip.className = "rp-resize"; grip.title = "Resize";
        grip.addEventListener("pointerdown", function (e) { resize(e, el, grip); });
        el.appendChild(grip);
      }
    }
    function resize(e, el, grip) {
      e.preventDefault(); e.stopPropagation();
      var at = find(el.dataset.rpCode); if (!at) return;
      var column = grid.getBoundingClientRect().width / COLUMNS;
      var startX = e.clientX, startWidth = at.item.width;
      var most = COLUMNS - used(rows[at.row], at.item);
      grip.setPointerCapture(e.pointerId); el.draggable = false;
      function move(ev) {
        var width = Math.max(MIN, Math.min(most, startWidth + Math.round((ev.clientX - startX) / column)));
        if (width !== at.item.width) { at.item.width = width; draw(); }
      }
      function up() {
        grip.removeEventListener("pointermove", move); grip.removeEventListener("pointerup", up);
        el.draggable = true;
      }
      grip.addEventListener("pointermove", move); grip.addEventListener("pointerup", up);
    }

    function start() {
      read();
      grid.classList.add("rp-editing");
      bar.querySelector("[data-rp-edit]").hidden = true;
      bar.querySelector(".rp-editbar__editing").hidden = false;
      grid.querySelectorAll(".rp-widget").forEach(function (el) { el.draggable = true; tools(el); });
      draw();
    }

    grid.addEventListener("dragstart", function (e) {
      var el = e.target.closest && e.target.closest(".rp-widget");
      if (!el || !grid.classList.contains("rp-editing")) return;
      dragged = find(el.dataset.rpCode).item; before = copy();
      e.dataTransfer.effectAllowed = "move";
      try { e.dataTransfer.setData("text/plain", dragged.code); } catch (err) { /* some browsers refuse */ }
      requestAnimationFrame(function () { el.classList.add("rp-dragged"); });
    });
    grid.addEventListener("dragover", function (e) {
      if (!dragged) return;
      e.preventDefault();
      var over = e.target.closest && e.target.closest(".rp-widget, .rp-newrow");
      if (!over || over === dragged.el) return;
      var from = find(dragged.code);
      if (over === strip) {
        if (rows[from.row].length === 1 && from.row === rows.length - 1) return;
        rows[from.row].splice(from.index, 1); rows.push([dragged]); draw(); return;
      }
      var to = find(over.dataset.rpCode); if (!to) return;
      var same = from.row === to.row;
      if (!same && used(rows[to.row]) + dragged.width > COLUMNS) { refuse(over); return; }
      var box = over.getBoundingClientRect();
      var after = e.clientX > box.left + box.width / 2;
      rows[from.row].splice(from.index, 1);
      to = find(over.dataset.rpCode);
      rows[to.row].splice(to.index + (after ? 1 : 0), 0, dragged);
      draw();
    });
    grid.addEventListener("drop", function (e) { if (dragged) { e.preventDefault(); before = null; } });
    grid.addEventListener("dragend", function () {
      if (!dragged) return;
      dragged.el.classList.remove("rp-dragged");
      if (before) { rows = before; draw(); }
      dragged = null; before = null;
    });

    bar.querySelector("[data-rp-edit]").addEventListener("click", start);
    bar.querySelector("[data-rp-cancel]").addEventListener("click", function () { location.reload(); });
    bar.addEventListener("click", function (e) {
      var button = e.target.closest && e.target.closest("[data-rp-scope]");
      if (!button) return;
      bar.elements.scope.value = button.dataset.rpScope;
      bar.elements.layout.value = JSON.stringify({
        rows: rows.map(function (r) { return r.map(function (w) { return { code: w.code, width: w.width }; }); }),
        hidden: hidden,
      });
    });
  }

  function scan() {
    document.querySelectorAll(".rp-grid").forEach(watch);
    document.querySelectorAll("[data-rp-editbar]").forEach(edit);
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", scan);
  else scan();
  document.addEventListener("htmx:load", scan);
})();
