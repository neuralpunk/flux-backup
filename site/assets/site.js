// Flux Backup website: theme switch, copy buttons, comment colouring, guide contents.
// Everything here is an enhancement; the pages read fine without JavaScript.

(function () {
  "use strict";

  // ---------- Theme: Dark / Light / Auto (the same three choices as the app) ----------

  var KEY = "flux-site-theme";
  var root = document.documentElement;

  function storedTheme() {
    try {
      var v = localStorage.getItem(KEY);
      return v === "dark" || v === "light" ? v : "auto";
    } catch (e) {
      return "auto";
    }
  }

  function applyTheme(choice) {
    if (choice === "auto") root.removeAttribute("data-theme");
    else root.setAttribute("data-theme", choice);
    document.querySelectorAll(".theme-switch button").forEach(function (b) {
      b.setAttribute("aria-pressed", String(b.dataset.theme === choice));
    });
  }

  applyTheme(storedTheme());

  document.querySelectorAll(".theme-switch").forEach(function (sw) {
    sw.classList.add("ready");
    sw.addEventListener("click", function (e) {
      var btn = e.target.closest("button[data-theme]");
      if (!btn) return;
      var choice = btn.dataset.theme;
      try {
        if (choice === "auto") localStorage.removeItem(KEY);
        else localStorage.setItem(KEY, choice);
      } catch (err) {
        /* storage blocked: the choice lasts for this page only */
      }
      applyTheme(choice);
    });
  });

  // ---------- Code blocks: dim comments, add a copy button ----------

  // Colour "# comment" in shell and TOML blocks, ignoring # inside quotes.
  function commentStart(line) {
    var quote = null;
    for (var i = 0; i < line.length; i++) {
      var ch = line[i];
      if (quote) {
        if (ch === "\\") i++;
        else if (ch === quote) quote = null;
      } else if (ch === '"' || ch === "'") {
        quote = ch;
      } else if (ch === "#" && (i === 0 || /\s/.test(line[i - 1]))) {
        return i;
      }
    }
    return -1;
  }

  function escapeHtml(s) {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  var copyIcon =
    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><rect x="5" y="5" width="8.5" height="8.5" rx="2"/><path d="M11 3.2V3a1.5 1.5 0 0 0-1.5-1.5H3A1.5 1.5 0 0 0 1.5 3v6.5A1.5 1.5 0 0 0 3 11h.2"/></svg>';
  var doneIcon =
    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3.5 8.5l3 3 6-7"/></svg>';

  document.querySelectorAll(".code pre code").forEach(function (code) {
    var lang = code.dataset.lang;
    var raw = code.textContent;

    if (lang === "sh" || lang === "toml") {
      code.innerHTML = raw
        .split("\n")
        .map(function (line) {
          var at = commentStart(line);
          if (at < 0) return escapeHtml(line);
          return escapeHtml(line.slice(0, at)) + '<span class="c">' + escapeHtml(line.slice(at)) + "</span>";
        })
        .join("\n");
    }

    if (!navigator.clipboard) return;
    var btn = document.createElement("button");
    btn.type = "button";
    btn.className = "copy-btn";
    btn.setAttribute("aria-label", "Copy to clipboard");
    btn.innerHTML = copyIcon;
    btn.addEventListener("click", function () {
      navigator.clipboard.writeText(raw.replace(/\n$/, "")).then(function () {
        btn.classList.add("done");
        btn.innerHTML = doneIcon;
        btn.setAttribute("aria-label", "Copied");
        setTimeout(function () {
          btn.classList.remove("done");
          btn.innerHTML = copyIcon;
          btn.setAttribute("aria-label", "Copy to clipboard");
        }, 1600);
      });
    });
    code.closest(".code").appendChild(btn);
  });

  // ---------- Guide: highlight the section being read ----------

  // The active section is the last one whose heading has passed under the sticky header;
  // at the very bottom of the page it is the last section, however short.
  var tocLinks = document.querySelectorAll(".toc a[href^='#']");
  if (tocLinks.length) {
    var byId = {};
    tocLinks.forEach(function (a) {
      byId[a.getAttribute("href").slice(1)] = a;
    });
    var sections = Array.prototype.filter.call(document.querySelectorAll(".guide section[id]"), function (s) {
      return byId[s.id];
    });
    var toc = document.querySelector(".toc");
    var currentId = null;
    var LINE = 120;

    function update() {
      var current = sections[0];
      var atBottom = window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 4;
      if (atBottom) {
        current = sections[sections.length - 1];
      } else {
        for (var i = 0; i < sections.length; i++) {
          if (sections[i].getBoundingClientRect().top <= LINE) current = sections[i];
          else break;
        }
      }
      if (current.id === currentId) return;
      currentId = current.id;
      tocLinks.forEach(function (a) {
        a.classList.toggle("active", a === byId[currentId]);
      });
      // Keep the active link visible when the contents list itself scrolls.
      var active = byId[currentId];
      var top = active.offsetTop;
      if (top < toc.scrollTop || top > toc.scrollTop + toc.clientHeight - 40) {
        toc.scrollTop = top - toc.clientHeight / 3;
      }
    }

    window.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    update();
  }

  // Close the mobile contents list after picking a section.
  document.querySelectorAll(".toc-mobile a").forEach(function (a) {
    a.addEventListener("click", function () {
      a.closest("details").open = false;
    });
  });
})();
