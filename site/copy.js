(() => {
  const root = document.documentElement;
  const timers = new WeakMap();

  root.classList.add("copy-enabled");

  const showState = (button, label, state) => {
    const activeTimer = timers.get(button);
    if (activeTimer) {
      window.clearTimeout(activeTimer);
    }

    button.setAttribute("aria-label", label);
    button.dataset.state = state;

    timers.set(button, window.setTimeout(() => {
      button.setAttribute("aria-label", button.dataset.copyLabel);
      button.dataset.state = "idle";
      timers.delete(button);
    }, 1500));
  };

  const selectText = (target) => {
    const selection = window.getSelection();
    if (!selection) {
      return;
    }

    const range = document.createRange();
    range.selectNodeContents(target);
    selection.removeAllRanges();
    selection.addRange(range);
  };

  const fallbackCopy = (text) => {
    const textarea = document.createElement("textarea");
    textarea.value = text;
    textarea.setAttribute("readonly", "");
    textarea.style.position = "fixed";
    textarea.style.inset = "-9999px auto auto -9999px";
    document.body.append(textarea);
    textarea.select();

    let copied = false;
    try {
      copied = document.execCommand("copy");
    } catch {
      copied = false;
    }

    textarea.remove();
    return copied;
  };

  document.querySelectorAll("[data-copy-target]").forEach((button) => {
    button.dataset.copyLabel = button.getAttribute("aria-label") || "Copy command";

    button.addEventListener("click", async () => {
      const target = document.getElementById(button.dataset.copyTarget);
      if (!target) {
        return;
      }

      const text = target.textContent.trim();
      let copied = false;

      if (navigator.clipboard && navigator.clipboard.writeText) {
        try {
          await navigator.clipboard.writeText(text);
          copied = true;
        } catch {
          copied = fallbackCopy(text);
        }
      } else {
        copied = fallbackCopy(text);
      }

      if (copied) {
        showState(button, `${button.dataset.copyLabel}: copied`, "copied");
      } else {
        selectText(target);
        showState(button, `${button.dataset.copyLabel}: command selected for manual copying`, "selected");
      }
    });
  });
})();
