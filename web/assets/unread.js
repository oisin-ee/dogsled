// A receipt belongs to the exact reply in this browser, not to a pane selection.
// The portal supplies matching session and transcript identity before mounting a read edge.
(() => {
  const event = "dogsled:reply-seen";
  const prefix = "dogsled:seen-reply:";
  const memory = new Set();
  function seen(key) {
    if (memory.has(key)) return true;
    try { return localStorage.getItem(prefix + key) !== null; }
    catch { return false; }
  }
  function mark(key) {
    if (!key || seen(key)) return;
    memory.add(key);
    try { localStorage.setItem(prefix + key, "1"); }
    catch { /* With storage disabled, receipts last for this open portal. */ }
    window.dispatchEvent(new Event(event));
  }
  class UnreadDot extends HTMLElement {
    static observedAttributes = ["data-receipt"];
    update = () => this.toggleAttribute("data-unread", !!this.dataset.receipt && !seen(this.dataset.receipt));
    connectedCallback() {
      window.addEventListener(event, this.update);
      window.addEventListener("storage", this.update);
      this.update();
    }
    attributeChangedCallback() { this.update(); }
    disconnectedCallback() {
      window.removeEventListener(event, this.update);
      window.removeEventListener("storage", this.update);
    }
  }
  class ReadEdge extends HTMLElement {
    // Always get a fresh intersection after refocus: a hidden tab may have changed scroll position.
    observe = () => {
      this.observer?.disconnect();
      this.observer = undefined;
      if (document.visibilityState !== "visible" || !document.hasFocus() || !this.isConnected) return;
      const observer = new IntersectionObserver(entries => {
        if (this.observer !== observer || !this.isConnected || !entries.at(-1)?.isIntersecting ||
            document.visibilityState !== "visible" || !document.hasFocus()) return;
        mark(this.dataset.receipt);
        observer.disconnect();
      });
      this.observer = observer;
      observer.observe(this);
    };
    connectedCallback() {
      window.addEventListener("focus", this.observe);
      document.addEventListener("visibilitychange", this.observe);
      this.observe();
    }
    disconnectedCallback() {
      window.removeEventListener("focus", this.observe);
      document.removeEventListener("visibilitychange", this.observe);
      this.observer?.disconnect();
      this.observer = undefined;
    }
  }
  customElements.define("dogsled-unread-dot", UnreadDot);
  customElements.define("dogsled-read-edge", ReadEdge);
})();
