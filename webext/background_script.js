// On startup, setup.
browser.runtime.onInstalled.addListener(async () => {
    console.log("keep-it-focused", "setup", "starting");
    await InterdictionManager.init();
    await ConfigManager.update();
    console.log("keep-it-focused", "setup", "complete");
});

// Rather than downloading the updated list every minute or so, even when the
// user is not in front of the computer, we expect any interaction with the user,
// and if we haven't downloaded the updated list in a while, we trigger an update.
browser.tabs.onUpdated.addListener(async () => {
    console.log("keep-it-focused", "event detected, do we need to update?");
    await ConfigManager.update();
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
    _interdictions: new Map(),

    _rules: null,

    // Initialize the interdiction manager.
    async init() {
        let rules = await browser.declarativeNetRequest.getSessionRules();
        for (let rule of rules) {
            if (rule.id >= this._counter) {
                this._counter = rule.id + 1;
            }
        }
        this._rules = rules;
    },

    // Add an interdiction.
    //
    // Don't forget to call `flush()`!
    addInterdiction(domain) {
        console.log("keep-it-focused", "InterdictionManager", "adding interdiction", domain);
        for (let rule of this._rules) {
            if (rule.condition.urlFilter == domain) {
                console.log("keep-it-focused", "InterdictionManager", "this interdiction is already in progress, skipping");
                return;
            }
        }
        let interdiction = new Interdiction(domain);
        this._addRules.push({
            action: {
                type: "block"
            },
            condition: {
                urlFilter: interdiction.domain
            },
            id: interdiction.id,
        });
        this._interdictions.set(interdiction.domain, interdiction);
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
        this._interdictions.delete(interdiction.domain);
    },

    // Flush any interdiction added/removed since the latest flush.
    async flush() {
        console.log("keep-it-focused", "InterdictionManager", "rules before flush", await browser.declarativeNetRequest.getSessionRules());
        let update = {
            addRules: this._addRules,
            removeRuleIds: this._removeRuleIds,
        };
        console.log("keep-it-focused", "InterdictionManager", "flushing", update);
        await browser.declarativeNetRequest.updateSessionRules(update);
        this._addRules.length = 0;
        this._removeRuleIds.length = 0;
        console.log("keep-it-focused", "InterdictionManager", "rules after flush", await browser.declarativeNetRequest.getSessionRules());
    },

    // The current list of interdictions. Please do not modify this.
    interdictions() {
        return this._interdictions
    }
};

// A domain (or domain regex) to interdict.
class Interdiction {
    // domain: string - the domain to which this rule applies
    constructor(domain) {
        this.domain = domain;
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
        for (let [domain, interdiction] of InterdictionManager.interdictions()) {
            console.log("keep-it-focused", "ConfigManager", "checking interdiction for", domain);
            let instructions = this._config.get(domain);
            (function() {
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
            }())
        }

        // Do we need to add new interdictions?
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

        // Do we need to update notifications?
        console.debug("keep-it-focused", "ConfigManager", "permissions in progress", permissionsInProgress);
        for (let [domain, interval] of permissionsInProgress) {
            let remaining = interval.contains(now);
            let message;
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
            } else if (remaining < FIVE_MINUTES_MS) {
                message = `Less than 5 minutes left for ${domain}!`;
                progress = 100;
            }
            if (message) {
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