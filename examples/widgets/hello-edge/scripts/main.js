// Hello Edge: exercises the iCUE globals EdgeHost injects (M1).
icueEvents = {
	onICUEInitialized: onInitialized,
	onDataUpdated: render,
};

const $ = (id) => document.getElementById(id);

function loadState() {
	try {
		return JSON.parse(localStorage.getItem(uniqueId) || "{}");
	} catch (_) {
		return {};
	}
}

function saveState(state) {
	localStorage.setItem(uniqueId, JSON.stringify(state));
}

function render() {
	const root = document.documentElement.style;
	root.setProperty("--fg", textColor);
	root.setProperty("--accent", accentColor);
	root.setProperty("--bg", backgroundColor);
	root.setProperty("--scale", String(fontScale / 100));
	$("headline").textContent = headline ?? "";
}

function tick() {
	const opts = { hour: "2-digit", minute: "2-digit", ...(showSeconds ? { second: "2-digit" } : {}) };
	$("clock").textContent = new Date().toLocaleTimeString(iCUE.iCUELanguage, opts);
}

function showFacts(state) {
	const facts = {
		viewport: `${window.innerWidth}×${window.innerHeight}`,
		language: iCUE.iCUELanguage,
		preview: String(iCUE.isPreview),
		"temperature unit": iCUE.defaultTemperatureUnit(),
		deviceId: device.deviceId,
		uniqueId,
		"times opened": String(state.opens),
		taps: String(state.taps),
	};
	$("facts").replaceChildren(
		...Object.entries(facts).flatMap(([k, v]) => {
			const dt = document.createElement("dt");
			const dd = document.createElement("dd");
			dt.textContent = k;
			dd.textContent = v;
			return [dt, dd];
		}),
	);
}

async function onInitialized() {
	const state = { opens: 0, taps: 0, ...loadState() };
	state.opens += 1;
	saveState(state);

	render();
	tick();
	setInterval(tick, 1000);
	showFacts(state);
	window.addEventListener("resize", () => showFacts(state));

	$("greeting").textContent = await tr("Rendered by EdgeHost");
	$("tap").addEventListener("click", () => {
		state.taps += 1;
		saveState(state);
		showFacts(state);
	});
	console.log("hello-edge initialized", `${window.innerWidth}x${window.innerHeight}`, JSON.stringify({ headline, textColor, fontScale, showSeconds, opens: state.opens, lang: iCUE.iCUELanguage }));
}

if (iCUE_initialized) {
	onInitialized();
}
