/* racer: page behaviour. No dependencies, no trackers, no network. */
(() => {
  "use strict";

  // ---------------------------------------------------------- copy button
  document.querySelectorAll(".copy").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const src = document.querySelector(btn.dataset.copy);
      if (!src) return;
      const label = btn.textContent;
      try {
        await navigator.clipboard.writeText(src.innerText.trim());
        btn.textContent = "Copied";
      } catch {
        // No clipboard (an insecure origin, or permission refused): select
        // the text so a long-press or Ctrl+C still works.
        btn.textContent = "Select it";
        const range = document.createRange();
        range.selectNodeContents(src);
        const sel = window.getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
      }
      setTimeout(() => (btn.textContent = label), 1600);
    });
  });

  // ------------------------------------------------- the APK on a phone
  // The only build is an Android one; on anything else, say so beside the
  // button rather than hide it.
  if (!/Android/i.test(navigator.userAgent)) {
    document.querySelector(".dl .card")?.classList.add("elsewhere");
  }

  // ------------------------------------------------------------ scrollspy
  const links = [...document.querySelectorAll('.topbar nav a[href^="#"]')];
  const targets = links
    .map((a) => document.querySelector(a.getAttribute("href")))
    .filter(Boolean);

  if (targets.length && "IntersectionObserver" in window) {
    const io = new IntersectionObserver(
      (entries) => {
        entries.forEach((e) => {
          if (!e.isIntersecting) return;
          links.forEach((a) => {
            if (a.getAttribute("href") === "#" + e.target.id) {
              a.setAttribute("aria-current", "true");
            } else {
              a.removeAttribute("aria-current");
            }
          });
        });
      },
      { rootMargin: "-45% 0px -50% 0px" }
    );
    targets.forEach((t) => io.observe(t));
  }
})();
