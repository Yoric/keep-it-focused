// On startup, setup.
browser.runtime.onInstalled.addListener(async () => {
    try {
        console.log("keep-it-focused", "setup", "starting");
        await InterdictionManager.init();
        console.log("keep-it-focused", "setup", "launching first update");
        await ConfigManager.update();
        console.log("keep-it-focused", "setup", "complete");
    } catch (ex) {
        console.error("keep-it-focused", "setup", "error", ex);
    }
});

// Every one minute, check for updates.
browser.alarms.create(
    "time to update", {
        periodInMinutes: 1,
    }
)

browser.alarms.onAlarm.addListener(async () => {
    try {
        console.debug("keep-it-focused", "tick", "start")
        await ConfigManager.update();
        console.debug("keep-it-focused", "tick", "stop")
    } catch (ex) {
        console.error("keep-it-focused", "tick", "error", ex);
    }
});

// The minimal delay between two updates, in ms.
const UPDATE_DELAY_MS = 1000 * 60;
const ONE_MINUTE_MS = 1000 * 60;
const TWO_MINUTES_MS = 1000 * 60 * 2;
const THREE_MINUTES_MS = 1000 * 60 * 3;
const FOUR_MINUTES_MS = 1000 * 60 * 4;
const FIVE_MINUTES_MS = 1000 * 60 * 5;

// The list of interdictions.
let InterdictionManager = {
    // A counter for rule IDs. Each rule needs a unique numeric id >= 1, so we seed it here.
    // In `init()`, we increment this in case we already have rules in progress
    // (e.g. because we're currently testing the add-on).
    _counter: 1,

    // The list of rules to add. Flushed by `flush()`.
    // @type Rules[]
    _addRules: [],

    // The list of rules to remove. Flushed by `flush()`.
    _removeRuleIds: [],

    // The current list of rules.
    //
    // domain => Interdiction
    _interdictionsByDomain: new Map(),

    // The latest version of the declarativeNetRequest rules understood by the browser.
    _rules: [],
    _rulesByDomain: new Map(),

    // A cached array of url filters, used to find whether a tab is breaking a rule.
    _urlFilters: null,

    // From a list of declarativeNetRequest rules, compute a map domain => rule. 
    _computeRulesByDomain(rules) {
        let rulesByDomain = new Map();
        for (let rule of rules) {
            if (rule.action.type != "block") {
                continue;
            }
            if (!rule.condition.urlFilter) {
                continue;
            }
            let re = /||(.*)/;
            let match = re.exec(rule.condition.urlFilter);
            if (!match) {
                continue;
            }
            rulesByDomain.set(match[1], rule);
        }
        return rulesByDomain;
    },

    // Initialize the interdiction manager.
    async init() {
        let rules = await browser.declarativeNetRequest.getSessionRules();
        console.log("keep-it-focused", "InterdictionManager", "existing rules", rules);
        for (let rule of rules) {
            if (rule.id >= this._counter) {
                this._counter = rule.id + 1;
            }
        }
        this._rules = rules;
        this._rulesByDomain = this._computeRulesByDomain(rules);
        // Compute an inital (empty) list of url filters.
        this.urlFilters();
    },

    // Add an interdiction.
    //
    // Don't forget to call `flush()`!
    addInterdiction(domain, interval) {
        console.log("keep-it-focused", "InterdictionManager", "adding interdiction", domain, "to", this._rules);
        let interdiction;
        let shouldAddRule;
        if (interdiction = this._interdictionsByDomain.get(domain)) {
            console.log("keep-it-focused", "InterdictionManager", "this interdiction is already in progress, updating interval");
            interdiction.interval = interval;
            shouldAddRule = false;
        } else {
            interdiction = new Interdiction(domain, interval);
            this._interdictionsByDomain.set(interdiction.domain, interdiction);
            shouldAddRule = true;
        }
        this._urlFilters = null; // We'll need to recompute url filters.
        if (this._rulesByDomain.get(domain)) {
            // This can happen e.g. when debugging an extension.
            console.log("keep-it-focused", "InterdictionManager", "we already have a rule for this interdiction, skipping");
            return;
        }
        if (shouldAddRule) {
            this._addRules.push({
                action: {
                    type: "block"
                },
                condition: {
                    urlFilter: "||" + interdiction.domain
                },
                id: interdiction.id,
            });    
        }
    },

    // Remove an interdiction.
    //
    // Don't forget to call `flush()`!
    removeInterdiction(interdiction) {
        console.log("keep-it-focused", "InterdictionManager", "removing interdiction", interdiction);
        if (!(interdiction instanceof Interdiction)) {
            throw new TypeError();
        }
        this._removeRuleIds.push(interdiction.id);
        this._interdictionsByDomain.delete(interdiction.domain);
        this._urlFilters = null; // We'll need to recompute url filters.
    },

    // Flush any interdiction added/removed since the latest flush.
    async flush() {
        console.log("keep-it-focused", "InterdictionManager", "rules before flush", this._rules);
        let update = {
            addRules: this._addRules,
            removeRuleIds: this._removeRuleIds,
        };
        if (update.addRules.length != 0 || update.removeRuleIds.length != 0) {
            console.log("keep-it-focused", "InterdictionManager", "flushing", update);
            await browser.declarativeNetRequest.updateSessionRules(update);
            this._rules = await browser.declarativeNetRequest.getSessionRules();
            this._rulesByDomain = this._computeRulesByDomain(this._rules);
            console.log("keep-it-focused", "InterdictionManager", "rules after flush", "=>", this._rules);    
        }

        // Now unload tabs.
        console.debug("keep-it-focused", "InterdictionManager", "time to unload tabs");
        let offendingTabs = await this.findOffendingTabs();
        console.debug("keep-it-focused", "InterdictionManager", "time to unload tabs", offendingTabs);
        for (let { tab } of offendingTabs) {
            await this.unloadTab(tab);
        }

        this._addRules.length = 0;
        this._removeRuleIds.length = 0;
    },

    async unloadTab(tab) {
        console.debug("keep-it-focused", "InterdictionManager", "unloading tab", tab, tab.id);
        let id = tab.id;
        if (typeof id != "number") {
            throw new TypeError("invalid tab id: " + id);
        }
        await browser.tabs.update(id, {
            url: "about:blank",
            autoDiscardable: true,
        })
        console.debug("keep-it-focused", "InterdictionManager", "tab unloaded");
    },

    // The current list of domain -> interdiction. Please do not modify this.
    interdictions() {
        return this._interdictionsByDomain
    },

    urlFilters() {
        if (!this._urlFilters) {
            browser.tabs.onUpdated.removeListener(this._tabListener);
            this._urlFilters = [...this._interdictionsByDomain.keys()
                .map((k) => `*://*.${k}/*`)];
            console.log("keep-it-focused", "InterdictionManager", "recomputed url filters", this._urlFilters);
            if (this._urlFilters.length > 0) {
                browser.tabs.onUpdated.addListener(this._tabListener, {
                    urls: this._urlFilters,
                    properties: ["url"],
                });    
            }
            console.debug("keep-it-focused", "InterdictionManager", "added listeners", this._urlFilters);
        }
        console.debug("keep-it-focused", "InterdictionManager", "url filters", this._urlFilters);
        return this._urlFilters
    },

    _tabListener(tabId, change, tab) {
        // Block from navigating to a forbidden URL.
        console.debug("keep-it-focused", "InterdictionManager", "tab attempting to navigate to unwanted url", change, tab);
        browser.tabs.update(tabId, {
            url: "about:blank"
        })
    },

    // Return the list of {tab} for tabs currently visiting a forbidden domain.
    async findOffendingTabs() {
        let urlFilters = this.urlFilters();
        console.debug("keep-it-focused", "InterdictionManager", "checking for offending tabs", urlFilters);
        if (urlFilters.length == 0) {
            return []
        }
        let currentTabs = await browser.tabs.query({
            url: urlFilters
        });
        console.debug("keep-it-focused", "InterdictionManager", "offending tabs", currentTabs);
        if (currentTabs.length > 0) {
            console.log("keep-it-focused", "InterdictionManager", "found offending tabs", currentTabs);
        } else {
            console.debug("keep-it-focused", "InterdictionManager", "no offending tabs");
        }
        return [...currentTabs.map((tab) => ({ tab }))];
    }

};

// A domain (or domain regex) to interdict.
class Interdiction {
    // domain: string - the domain to which this rule applies
    constructor(domain, interval) {
        this.domain = domain;
        this.interval = interval;
        this.id = ++InterdictionManager._counter;
    }
}

// An interval of time.
class Interval {
    constructor(start, end) {
        if (!(start instanceof Date) || !(end instanceof Date)) {
            throw new TypeError();
        }
        this.start = start;
        this.end = end;
    }
    // Check if a date is contained within the interval.
    //
    // @return false if the date is not contained within the interval.
    // @return number the number of milliseconds remaining if the date is currently contained within the interval.
    contains(date) {
        if (!(date instanceof Date)) {
            throw new TypeError();
        }
        if (this.start <= date && this.end > date) {
            return this.end - date;
        }
        return false;
    }
}

let ConfigManager = {
    // Timestamp for the latest update.
    _latestUpdateTS: null,

    /**
     * The latest config
        config: {
            "website.com": [
                {"start": "HH:MM"},
                {"end": "HH:MM"}
            ]
        }
     */
    _config: null,

    // Promise|null
    //
    // Resolves when `refetchIfNecessary` completes.
    _lock: null,

    // Fetch rules if they haven't been fetched in a while, then update authorizations.
    update: async function () {
        if (this._lock) {
            console.log("keep-it-focused", "ConfigManager", "update already in progress");
            return;
        }

        console.log("keep-it-focused", "ConfigManager", "checking whether we need to update");
        this._lock = this._refetchIfNecessary();
        await this._lock;
        this._lock = null;
        console.debug("keep-it-focused", "ConfigManager", "update tango complete", this._config);

        let now = new Date();

        let permissionsInProgress = new Map();
        // Do we need to stop existing interdictions?
        console.debug("keep-it-focused", "ConfigManager", "looking for interdictions that have stopped");
        for (let [domain, interdiction] of InterdictionManager.interdictions()) {
            console.log("keep-it-focused", "ConfigManager", "checking interdiction for", domain);
            let instructions = this._config.get(domain);
            (function () {
                if (!instructions) {
                    // No instructions for a domain? It's permitted.
                    console.log("keep-it-focused", "ConfigManager", "interdiction for", domain, "has been removed, updating rules");
                    InterdictionManager.removeInterdiction(interdiction);
                    return;
                }

                for (let interval of instructions) {
                    if (interval.contains(now)) {
                        // This domain is currently permitted.
                        console.log("keep-it-focused", "ConfigManager", "interdiction for", domain, "has reached its end, updating rules");
                        InterdictionManager.removeInterdiction(interdiction);
                        permissionsInProgress.set(domain, interval);
                        return;
                    }
                }

                // Otherwise, let interdictions continue.
                console.log("keep-it-focused", "ConfigManager", "interdiction for", domain, "continues");
            }());
        }

        // Do we need to add new interdictions?
        console.debug("keep-it-focused", "ConfigManager", "looking for interdictions that have started");
        for (let [domain, intervals] of this._config) {
            let allowed = false;
            for (let interval of intervals) {
                console.debug("keep-it-focused", "ConfigManager", "is", domain, "currently permitted?", interval);
                if (interval.contains(now)) {
                    console.debug("keep-it-focused", "ConfigManager", domain, "is currently permitted", interval);
                    allowed = true;
                    permissionsInProgress.set(domain, interval);
                    break;
                }
            }
            if (allowed) {
                continue;
            }
            console.debug("keep-it-focused", "ConfigManager", domain, "is currently forbidden");
            InterdictionManager.addInterdiction(domain);
        }

        // Flush interdictions.
        await InterdictionManager.flush();
        console.debug("keep-it-focused", "ConfigManager", "permissions in progress", permissionsInProgress);

        // Do we need to notify?
        for (let [domain, interval] of permissionsInProgress) {
            let remaining = interval.contains(now);
            if (remaining < FIVE_MINUTES_MS) {
                // A permission interval is closing, do we need to notify?
                let tabs = await browser.tabs.query({
                    active: true,
                    url: `*://*.${domain}/*`,
                });
                console.debug("keep-it-focused", "ConfigManager", "looking for activity that needs to stop", domain, tabs);
                if (tabs.length == 0) {
                    // No such tabs, no need to notify.
                    continue;
                }
                let progress = 1;
                if (remaining < ONE_MINUTE_MS) {
                    message = `Less than one minute left for ${domain}!`;
                    progress = 20;
                } else if (remaining < TWO_MINUTES_MS) {
                    message = `Less than 2 minutes left for ${domain}!`;
                    progress = 40;
                }  else if (remaining < THREE_MINUTES_MS) {
                    message = `Less than 3 minutes left for ${domain}!`;
                    progress = 60;
                } else if (remaining < FOUR_MINUTES_MS) {
                    message = `Less than 4 minutes left for ${domain}!`;
                    progress = 80;
                } else {
                    message = `Less than 5 minutes left for ${domain}!`;
                    progress = 100;
                }
                browser.notifications.create({
                    type: "progress",
                    title: "Keep it Focused",
                    message,
                    progress,
                });
            }
        }
    },

    // Fetch instructions if they haven't been fetched in a while.
    _refetchIfNecessary: async function () {
        let now = Date.now();
        if (this._latestUpdateTS != null && now - this.latestUpdateTS <= UPDATE_DELAY_MS) {
            // No need to refetch yet.
            console.log("keep-it-focused", "ConfigManager", "no need to update");
            return;
        }
        console.log("keep-it-focused", "ConfigManager", "update needed");
        try {
            let response = await fetch("http://localhost:7878", {
                method: "GET",
            });
            if (!response.ok) {
                console.error("keep-it-focused", "ConfigManager", "could not get in touch with update server, skipping this update");
                return;
            }
            let json = await response.json();
            console.log("keep-it-focused", "ConfigManager", "obtained update from server", json);

            // Convert times in HHMM to Date(), which are simpler to use.
            let config = new Map();
            for (let domain of Object.keys(json)) {
                let dateIntervals = [];
                for (let interval of json[domain]) {
                    console.debug("keep-it-focused", "ConfigManager", "looking at interval", interval);
                    let { start, end } = interval;
                    let dateStart = hhmmToDate(start);
                    let dateEnd = hhmmToDate(end);
                    dateIntervals.push(new Interval(dateStart, dateEnd));
                }
                config.set(domain, dateIntervals);
            }
            this._config = config;
            this._latestUpdateTS = now;
        } catch (ex) {
            console.error("keep-it-focused", "ConfigManager", "error during update", ex);
        }
    }
};

// A regex for times in HHMM format.
const HHMM = /(\d\d)(\d\d)/;

// Convert a time in HHMM to a date in today (or tomorrow).
//
// Conversions assume that HHMM uses the local time zone.
function hhmmToDate(source) {
    let captures = HHMM.exec(source);
    if (!captures) {
        throw new TypeError("invalid hhmm " + source);
    }
    let hh = captures[1];
    let mm = captures[2];
    let hours = Number.parseInt(hh);
    let minutes = Number.parseInt(mm);
    let date = new Date();
    date.setHours(hours);
    date.setMinutes(minutes);
    date.setSeconds(0);
    return date;
}