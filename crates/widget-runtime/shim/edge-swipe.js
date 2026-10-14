// EdgeHost page swipes: a quick horizontal drag across a widget or the background changes page.
//
// This file evaluates to a function of one argument, `post` (sends a message to EdgeHost); Rust
// appends the call (widget_runtime::surface::swipe_script) and injects it into every visible view
// on the surface. Distances are CSS pixels, which equal panel pixels because every view on the
// surface is zoomed by the panel scale.
//
// A drag is not a swipe when it starts in a text field or form control, or on an element that
// handles its own drags: CSS `touch-action: none` (or `pan-y` without `pan-x`) on it or an
// ancestor, as on piano keys or a scrubbable chart, or a `data-edgehost-no-swipe` attribute.
(function (post) {
	"use strict";

	// About 22 mm on the Edge's 2560-pixel-wide panel.
	const MIN_DISTANCE = 160;
	const MAX_DURATION_MS = 800;

	// 1 = next page (finger moves left), -1 = previous page, 0 = not a swipe.
	const classify = (dx, dy, ms) => {
		if (ms > MAX_DURATION_MS || Math.abs(dx) < MIN_DISTANCE || Math.abs(dx) < 2 * Math.abs(dy)) return 0;
		return dx < 0 ? 1 : -1;
	};

	const CONTROL = /^(INPUT|TEXTAREA|SELECT)$/;
	const ownsDrags = (el) => {
		if (typeof getComputedStyle !== "function") return false;
		const action = getComputedStyle(el).touchAction || "";
		return action === "none" || (action.includes("pan-y") && !action.includes("pan-x"));
	};
	const optedOut = (target) => {
		for (let el = target; el && el.nodeType === 1; el = el.parentElement) {
			if (CONTROL.test(el.tagName) || el.isContentEditable) return true;
			if (el.hasAttribute("data-edgehost-no-swipe") || ownsDrags(el)) return true;
		}
		return false;
	};

	let start = null;
	let swallowClick = false;
	window.addEventListener(
		"pointerdown",
		(e) => {
			start = e.isPrimary && e.button === 0 && !optedOut(e.target) ? { x: e.clientX, y: e.clientY, t: e.timeStamp } : null;
		},
		true,
	);
	window.addEventListener("pointercancel", () => (start = null), true);
	window.addEventListener(
		"pointerup",
		(e) => {
			if (!start) return;
			const dir = classify(e.clientX - start.x, e.clientY - start.y, e.timeStamp - start.t);
			start = null;
			if (dir === 0) return;
			// A swipe that ends on the element it started on would also click it.
			swallowClick = true;
			setTimeout(() => (swallowClick = false), 0);
			post({ k: "swipe", dir });
		},
		true,
	);
	window.addEventListener(
		"click",
		(e) => {
			if (!swallowClick) return;
			swallowClick = false;
			e.stopPropagation();
			e.preventDefault();
		},
		true,
	);

	return { classify, optedOut };
})
