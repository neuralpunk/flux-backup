// Flux Backup website: copy buttons, the landing page's motion, the guide's contents.
// Everything here is an enhancement; the pages read fine without JavaScript.

(function () {
  "use strict";

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
    var LINE = 110;

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

  // ---------- Home: motion ----------
  // Reveals as things enter; the opening's progress (--p) as it scrolls; screens that
  // settle into place (--v); a paragraph whose words light as they pass the middle; and
  // numbers that count up once. All of it is skipped for reduced motion.

  var still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  if ("IntersectionObserver" in window && !still) {
    var reveal = new IntersectionObserver(
      function (entries) {
        entries.forEach(function (e) {
          if (e.isIntersecting) {
            e.target.classList.add("in");
            reveal.unobserve(e.target);
          }
        });
      },
      { threshold: 0.12 },
    );
    document.querySelectorAll(".r").forEach(function (el) {
      reveal.observe(el);
    });
  } else {
    document.querySelectorAll(".r").forEach(function (el) {
      el.classList.add("in");
    });
  }

  var words = [];
  document.querySelectorAll("[data-words]").forEach(function (p) {
    p.innerHTML = p.innerHTML.replace(/(<[^>]+>)|([^\s<]+)/g, function (m, tag, w) {
      return tag ? tag : '<span class="w">' + w + "</span>";
    });
    words = words.concat(Array.prototype.slice.call(p.querySelectorAll(".w")));
  });
  var rises = Array.prototype.slice.call(document.querySelectorAll("[data-rise]"));
  var scales = Array.prototype.slice.call(document.querySelectorAll("[data-scale]"));

  function frame() {
    var vh = window.innerHeight;
    rises.forEach(function (s) {
      var r = s.getBoundingClientRect();
      var p = still ? 1 : Math.min(1, Math.max(0, -r.top / Math.max(1, r.height - vh)));
      s.style.setProperty("--p", p.toFixed(4));
    });
    scales.forEach(function (s) {
      var b = s.getBoundingClientRect();
      var v = still ? 1 : Math.min(1, Math.max(0, (vh - b.top) / (vh * 0.75)));
      s.style.setProperty("--v", v.toFixed(3));
    });
    words.forEach(function (w) {
      w.classList.toggle("lit", still || w.getBoundingClientRect().top < vh * 0.6);
    });
  }

  if (rises.length || scales.length || words.length) {
    var ticking = false;
    window.addEventListener(
      "scroll",
      function () {
        if (ticking) return;
        ticking = true;
        requestAnimationFrame(function () {
          ticking = false;
          frame();
        });
      },
      { passive: true },
    );
    window.addEventListener("resize", frame);
    frame();
  }

  // [data-go]: gets .go once in view (the speed bars run); its [data-count] numbers count up.
  var goers = document.querySelectorAll("[data-go]");
  if (goers.length && "IntersectionObserver" in window) {
    var counter = new IntersectionObserver(
      function (entries) {
        entries.forEach(function (e) {
          if (!e.isIntersecting) return;
          counter.unobserve(e.target);
          e.target.classList.add("go");
          if (still) return;
          e.target.querySelectorAll("[data-count]").forEach(function (el) {
            var to = parseFloat(el.dataset.count);
            var dp = (el.dataset.count.split(".")[1] || "").length;
            var t0 = performance.now();
            function step(now) {
              var k = Math.min(1, (now - t0) / 1500);
              var eased = 1 - Math.pow(1 - k, 4);
              el.textContent = (to * eased).toFixed(dp);
              if (k < 1) requestAnimationFrame(step);
            }
            requestAnimationFrame(step);
          });
        });
      },
      { threshold: 0.35 },
    );
    goers.forEach(function (el) {
      counter.observe(el);
    });
  } else {
    goers.forEach(function (el) {
      el.classList.add("go");
    });
  }

  // Close the mobile contents list after picking a section.
  document.querySelectorAll(".toc-mobile a").forEach(function (a) {
    a.addEventListener("click", function () {
      a.closest("details").open = false;
    });
  });
})();
