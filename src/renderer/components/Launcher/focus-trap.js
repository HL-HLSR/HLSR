"use strict";

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "summary",
  "[tabindex]:not([tabindex='-1'])",
].join(", ");

function isVisible(el) {
  return Boolean(el.offsetWidth || el.offsetHeight || el.getClientRects().length);
}

function focusableIn(root) {
  if (!root || !root.querySelectorAll) return [];
  return Array.prototype.filter.call(root.querySelectorAll(FOCUSABLE), isVisible);
}

function trapFocus(root, event) {
  const items = focusableIn(root);
  if (!items.length) return false;

  const first = items[0];
  const last = items[items.length - 1];
  const active = document.activeElement;

  if (!root.contains(active)) {
    event.preventDefault();
    (event.shiftKey ? last : first).focus();
    return true;
  }

  if (event.shiftKey && active === first) {
    event.preventDefault();
    last.focus();
    return true;
  }

  if (!event.shiftKey && active === last) {
    event.preventDefault();
    first.focus();
    return true;
  }

  return false;
}

module.exports = { trapFocus, focusableIn, FOCUSABLE };
