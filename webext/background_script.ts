// The minimal delay between two updates, in ms.
const UPDATE_DELAY_MS = 1000 * 60;
const ONE_MINUTE_MS = 1000 * 60;
const TWO_MINUTES_MS = 1000 * 60 * 2;
const THREE_MINUTES_MS = 1000 * 60 * 3;
const FOUR_MINUTES_MS = 1000 * 60 * 4;
const FIVE_MINUTES_MS = 1000 * 60 * 5;

class TimeManager {
    // The current list of rules.
    //
    // domain => Authorization
    _authorizationsByDomain: Map<string /* domain */, Map<Interval, Authorization>> = new Map()
    _authorizationsByAlarmKeyEnter: Map<string /* key */, Authorization> = new Map();
    _authorizationsByAlarmKeyExit: Map<string /* key */, Authorization> = new Map();

    _domainsToCheck: Set<string> = new Set();

    init() {
        browser.alarms.onAlarm.addListener(this._onAlarm)
    }

    // Add an authorization.
    //
    // This triggers the entire machinery dedicated to
    // - add a rule currently forbidding the domain, if no authorization is currently valid;
    // - add a rule currently forbidden the do
    addInterval(domain: string, interval: Interval) {
        console.log("keep-it-focused", "TimeManager", "adding interval", domain, interval);
        let authorization = new Authorization(domain, interval);
        let byInterval = this._authorizationsByDomain.get(domain);
        if (!byInterval) {
            byInterval = new Map();
            this._authorizationsByDomain.set(domain, byInterval);
        }
        byInterval.set(interval, authorization);

        this._registerAlarms(authorization);

        // Also, check intervals immediately.
        this._domainsToCheck.add(domain);
    }

    _registerAlarms(authorization: Authorization) {
        // We need to be informed when we enter, prepare to leave or leave entirely the interval.
        // Note that there may be more than one alarm in flight for a domain.

        // Be informed when we're entering the authorization interval.
        //
        // If `interval.start` is in the past, this will never be triggered.
        let keyEnter = authorization.keyEnter();
        this._authorizationsByAlarmKeyEnter.set(keyEnter, authorization);
        console.debug("keep-it-focused", "TimeManager", "creating enter alarm for", authorization.domain, authorization.interval.start);
        browser.alarms.create(keyEnter, {
            when: authorization.interval.start.valueOf(),
        });

        // Be informed when we're about to leave the interval.
        //
        // If `warnMeAtTS` is in the past, this might never be triggered.
        let keyExit = authorization.keyExit();
        let warnMeAtTS = authorization.interval.end.valueOf() - 1000 * 60 * 5;
        this._authorizationsByAlarmKeyExit.set(keyExit, authorization);
        console.debug("keep-it-focused", "TimeManager", "creating startup alarm for", authorization.domain, authorization.interval.start);
        browser.alarms.create(keyExit, {
            when: warnMeAtTS,
            periodInMinutes: 1
        });
    }

    _unregisterAlarms(authorization: Authorization) {
        let [keyEnter, keyExit] = [authorization.keyEnter(), authorization.keyExit()];
        for (let key of [keyEnter, keyExit]) {
            browser.alarms.clear(key); // Clear the alarm in the background.
        }
        this._authorizationsByAlarmKeyEnter.delete(keyEnter);
        this._authorizationsByAlarmKeyExit.delete(keyExit);
    }

    removeInterval(domain: string, interval: Interval) {
        console.log("keep-it-focused", "TimeManager", "removing interval", domain, interval);
        let byInterval = this._authorizationsByDomain.get(domain);
        if (!byInterval) {
            throw new TypeError("No rule for domain " + domain);
        }
        let authorization = byInterval.get(interval);
        if (!authorization) {
            throw new TypeError("No rule for domain " + domain + " interval " + interval);
        }
        byInterval.delete(interval);
        if (byInterval.size == 0) {
            this._authorizationsByDomain.delete(domain);
        }
        this._unregisterAlarms(authorization);
    }

    async flush() {
        console.log("keep-it-focused", "TimeManager", "flushing");
        const domainsToCheck = this._domainsToCheck;
        this._domainsToCheck = new Set();
        for (let domain of domainsToCheck) {
            await this._checkDomain(domain);
        }
        console.log("keep-it-focused", "TimeManager", "flushing", "complete");
        await ruleManager.flush();
    }

    async _checkDomain(domain: string) {
        console.debug("keep-it-focused", "TimeManager", "_checkDomain", "checking whether", domain, "is permitted");
        let intervals = this._authorizationsByDomain.get(domain);
        if (!intervals) {
            console.debug("keep-it-focused", "TimeManager", "_checkDomain", domain, "no intervals found, bailing out");
            return;
        }

        // There could be race conditions between adding/removing rules and the alarm ringing.
        //
        // So we could have been registered to allow/forbid/warn for a domain but it turns out that we should not
        // allow/forbid/warn for it after all. For this reason, we look at all the currently valid intervals for
        // the domain and we use them to determine our policy.
        let now = new Date();
        let remains;
        for (let interval of intervals.keys()) {
            if (remains = interval.contains(now)) {
                break;
            }
        }

        // Should we display a warning?
        let tabs = browser.tabs.query({
            active: true,
            url: `*://*.${domain}/*`,
        });
        if (!remains) {
            // Domain is now forbidden.
            console.debug("keep-it-focused", "TimeManager", "_checkDomain", domain, "domain is now forbidden");
            ruleManager.forbidDomain(domain);

            console.debug("keep-it-focused", "TimeManager", "_checkDomain", domain, "domain is now forbidden", "unloading tabs");
            for (let tab of await tabs) {
                // Browser away, in the background.
                if (tab.id) {
                    browser.tabs.update(tab.id, {
                        url: "about:blank"
                    });
                }
            }

            console.debug("keep-it-focused", "TimeManager", "_checkDomain", domain, "domain is now forbidden", "we're done");
            return;
        }

        // Domain is now allowed.
        console.debug("keep-it-focused", "TimeManager", "_checkDomain", domain, "domain is now permitted");
        ruleManager.allowDomain(domain);
        await ruleManager.flush(); // Request flush, asynchronously.

        // Should we warn?
        if ((await tabs).length == 0) {
            console.debug("keep-it-focused", "TimeManager", "_checkDomain", domain, "domain is now permitted", "no tabs visiting this domain");
            return;
        }
        let message;
        let progress = 1;
        if (remains < ONE_MINUTE_MS) {
            message = `Less than one minute left for ${domain}!`;
            progress = 20;
        } else if (remains < TWO_MINUTES_MS) {
            message = `Less than two minute left for ${domain}!`;
            progress = 40;
        } else if (remains < THREE_MINUTES_MS) {
            message = `Less than three minute left for ${domain}!`;
            progress = 60;
        } else if (remains < FOUR_MINUTES_MS) {
            message = `Less than four minute left for ${domain}!`;
            progress = 80;
        } else if (remains < FIVE_MINUTES_MS) {
            message = `Less than two minute left for ${domain}!`;
            progress = 100;
        }
        if (message) {
            console.debug("keep-it-focused", "TimeManager", "_checkDomain", domain, "domain is currently permitted", "displaying warning", message);
            browser.notifications.create({
                type: "progress",
                title: "Keep it Focused",
                message,
                progress,
            });
        } else {
            console.debug("keep-it-focused", "TimeManager", "_checkDomain", domain, "domain is currently permitted", "no need to display warning");
        }
    }

    async _onAlarm(alarm: browser.alarms.Alarm) {
        console.debug("keep-it-focused", "TimeManager", "_onAlarm triggered", alarm);

        // True if we reach this point by entering the validity interval.
        // False if we reachi this point by exiting (or nearing the end of) the validity interval.
        let isEnter;

        let authorization = this._authorizationsByAlarmKeyEnter.get(alarm.name);
        if (authorization) {
            isEnter = true;
        } else {
            authorization = this._authorizationsByAlarmKeyExit.get(alarm.name);
            isEnter = false;
        }
        if (!authorization) {
            // This can happen if the interval has been removed while we were looking away.
            console.debug("keep-it-focused", "TimeManager", "_onAlarm", "no alarm found, bailing out");
            return;
        }

        let now = new Date();
        if (isEnter || authorization.interval.end <= now) {
            // Time to unregister this alarm.
            browser.alarms.clear(alarm.name);
        }

        await this._checkDomain(authorization.domain);
    }
}
const timeManager = new TimeManager();

/**
 * The list of authorizations.
 */
class RuleManager {
    // A counter for rule IDs. Each rule needs a unique numeric id >= 1, so we seed it here.
    // In `init()`, we increment this in case we already have rules in progress
    // (e.g. because we're currently testing the add-on).
    _counter: number = 1

    // The list of rules to add. Flushed by `flush()`.
    _addRules: browser.declarativeNetRequest.Rule[] = []

    // The list of rules to remove. Flushed by `flush()`.
    _removeRuleIds: number[] = []


    // The latest version of the declarativeNetRequest rules understood by the browser.
    _currentRules: browser.declarativeNetRequest.Rule[] = []
    _currentRulesByDomain: Map<string /* domain */, browser.declarativeNetRequest.Rule> = new Map()

    // A cached array of url filters, used to find whether a tab is breaking a rule.
    _urlFilters: string[] | null = null

    // From a list of declarativeNetRequest rules, compute a map domain => rule. 
    _computeRulesByDomain(rules: browser.declarativeNetRequest.Rule[]): Map<string/*domain*/, browser.declarativeNetRequest.Rule> {
        let rulesByDomain = new Map();
        for (let rule of rules) {
            console.debug("keep-it-focused", "RuleManager", "_computeRulesByDomain", "converting rule", rule);
            if (rule.action.type != "block") {
                continue;
            }
            if (!rule.condition.urlFilter) {
                continue;
            }
            let re = /\|\|(.*)/;
            let match = re.exec(rule.condition.urlFilter);
            console.debug("keep-it-focused", "RuleManager", "_computeRulesByDomain", "converting rule", rule, match);
            if (!match || !match[1]) {
                continue;
            }
            rulesByDomain.set(match[1], rule);
        }
        return rulesByDomain;
    }

    // Initialize the authorization manager.
    async init() {
        // Make sure that we start `_counter` high enough.
        //
        // This should not be necessary in production, but it is necessary when running the extension
        // with web-ext.
        let rules = await this._fetchSessionRules();
        console.log("keep-it-focused", "RuleManager", "existing rules", rules);
        for (let rule of rules) {
            if (rule.id >= this._counter) {
                this._counter = rule.id + 1;
            }
        }
        // Compute an inital (empty) list of url filters.
        this.urlFilters();
    }

    async _fetchSessionRules(): Promise<browser.declarativeNetRequest.Rule[]> {
        console.debug("keep-it-focused", "RuleManager", "fetching session rules");
        let rules = await browser.declarativeNetRequest.getSessionRules();
        this._currentRules = rules;
        this._currentRulesByDomain = this._computeRulesByDomain(rules);
        console.debug("keep-it-focused", "RuleManager", "fetching session rules", "=>", rules);
        return rules;
    }

    allowDomain(domain: string) {
        console.log("keep-it-focused", "RuleManager", "allowing domain", domain, this._currentRulesByDomain);
        let rule = this._currentRulesByDomain.get(domain);
        if (rule) {
            this._removeRuleIds.push(rule.id);
            console.log("keep-it-focused", "RuleManager", "allowing domain", domain, "allowing");
            this._urlFilters = null; // We'll need to recompute url filters.
        } else {
            console.debug("keep-it-focused", "RuleManager", "allowing domain", domain, "already allowed");
        }
    }

    forbidDomain(domain: string) {
        let rule = this._currentRulesByDomain.get(domain);
        if (rule) {
            console.log("keep-it-focused", "RuleManager", "we already have a rule for this authorization, skipping");
            return;
        }
        this._addRules.push({
            action: {
                type: "block"
            },
            condition: {
                urlFilter: "||" + domain
            },
            id: ++this._counter,
        });    
        this._urlFilters = null; // We'll need to recompute url filters.
    }

    // Flush any authorization added/removed since the latest flush.
    async flush() {
        console.log("keep-it-focused", "RuleManager", "rules before flush", this._currentRules);
        let update = {
            addRules: this._addRules,
            removeRuleIds: this._removeRuleIds,
        };
        this._addRules = []; 
        this._removeRuleIds = [];
        if (update.addRules.length != 0 || update.removeRuleIds.length != 0) {
            console.log("keep-it-focused", "RuleManager", "flushing", update);
            await browser.declarativeNetRequest.updateSessionRules(update);
            this._currentRules = await browser.declarativeNetRequest.getSessionRules();
            this._currentRulesByDomain = this._computeRulesByDomain(this._currentRules);
            console.log("keep-it-focused", "RuleManager", "rules after flush", "=>", this._currentRules);
        }
    }

    async unloadTab(tab: browser.tabs.Tab) {
        console.debug("keep-it-focused", "RuleManager", "unloading tab", tab, tab.id);
        let id = tab.id;
        if (typeof id != "number") {
            throw new TypeError("invalid tab id: " + id);
        }
        await browser.tabs.update(id, {
            url: "about:blank",
        })
        console.debug("keep-it-focused", "RuleManager", "tab unloaded");
    }

    // Recompute the list of offending urls.
    //
    // They typically look like `*://*.{domain name}/*`, to catch all accesses
    // to offending domain and subdomains.
    urlFilters() {
        if (!this._urlFilters) {
            browser.tabs.onUpdated.removeListener(this._tabListener);
            this._urlFilters = [...this._currentRulesByDomain.keys()]
                .map((k) => `*://*.${k}/*`);
            console.log("keep-it-focused", "RuleManager", "recomputed url filters", this._urlFilters);
            if (this._urlFilters.length > 0) {
                browser.tabs.onUpdated.addListener(this._tabListener, {
                    urls: this._urlFilters,
                    properties: ["url"],
                });    
            }
            console.debug("keep-it-focused", "RuleManager", "added listeners", this._urlFilters);
        }
        console.debug("keep-it-focused", "RuleManager", "url filters", this._urlFilters);
        return this._urlFilters
    }

    _tabListener(tabId: number, change: browser.tabs.ChangeInfo, tab: browser.tabs.Tab): void {
        // Block from navigating to a forbidden URL.
        console.debug("keep-it-focused", "RuleManager", "tab attempting to navigate to unwanted url", change, tab);
        browser.tabs.update(tabId, {
            url: "about:blank"
        })
    }

    // Return the list of {tab} for tabs currently visiting a forbidden domain.
    async findOffendingTabs() {
        let urlFilters = this.urlFilters();
        console.debug("keep-it-focused", "RuleManager", "checking for offending tabs", urlFilters);
        if (urlFilters.length == 0) {
            return []
        }
        let currentTabs = await browser.tabs.query({
            url: urlFilters
        });
        console.debug("keep-it-focused", "RuleManager", "offending tabs", currentTabs);
        if (currentTabs.length > 0) {
            console.log("keep-it-focused", "RuleManager", "found offending tabs", currentTabs);
        } else {
            console.debug("keep-it-focused", "RuleManager", "no offending tabs");
        }
        return [...currentTabs.map((tab) => ({ tab }))];
    }

}
const ruleManager = new RuleManager();



// A domain (or domain regex) to interdict.
class Authorization {
    interval: Interval
    domain: string

    // domain: string - the domain to which this rule applies
    constructor(domain: string, interval: Interval) {
        this.domain = domain;
        this.interval = interval;
    }

    keyEnter(): string {
        return `${this.domain}-${this.interval}-enter`;
    }

    keyExit(): string {
        return `${this.domain}-${this.interval}-exit`;
    }
}

// An interval of time.
class Interval {
    start: Date
    end: Date
    constructor(start: Date, end: Date) {
        this.start = start;
        this.end = end;
    }
    // Check if a date is contained within the interval.
    //
    // @return false if the date is not contained within the interval.
    // @return number the number of milliseconds remaining if the date is currently contained within the interval.
    contains(date: Date): number | false {
        if (this.start <= date && this.end > date) {
            return this.end.valueOf() - date.valueOf();
        }
        return false;
    }
    toString(): string {
        return `interval-${this.start}-${this.end}`;
    }
}

/**
 * The manager for configuration.
 *
 * This component is in charge of the loop that fetches the configuration from the daemon. Whenever the configuration
 * changes, it:
 * - informs the RuleManager;
 * - informs the user if necessary.
 */
class ConfigManager {
    // Timestamp for the latest update, in ms since the epoch.
    _latestUpdateTS: number|null = null

    /**
     * The latest config
        config: {
            "website.com": [
                {"start": "HH:MM"},
                {"end": "HH:MM"}
            ]
        }
     */
    _config: Map<string, Interval[]> = new Map()

    // Promise|null
    //
    // Resolves when `refetchIfNecessary` completes.
    _lock: null | Promise<Map<string, Interval[]>> = null

    // `true` when the webextension is uninstalled.
    _shutdownRequested: boolean = false

    async init() {
        this._shutdownRequested = false;
        // Update immediately, then loop in the background.
        console.info("keep-it-focused", "ConfigManager", "startup update", "start");
        await this._update({ immediate: true });
        console.info("keep-it-focused", "ConfigManager", "startup update", "complete");
        let loop = async () => {
            console.info("keep-it-focused", "ConfigManager", "background update", "starting");
            while (!this._shutdownRequested) {
                // Permanently, wait for updates.
                //
                // We count on the server to respond slowly.
                try {
                    console.info("keep-it-focused", "ConfigManager", "background update", "start");
                    await this._update();
                    console.info("keep-it-focused", "ConfigManager", "background update", "complete");
                } catch (ex) {
                    console.error("keep-it-focused", "ConfigManager", "background error", ex);
                }
            }
            console.info("keep-it-focused", "ConfigManager", "background update", "shutdown");
        }
        loop();
    }

    suspend() {
        this._shutdownRequested = true;  
    }

    // Fetch rules if they haven't been fetched in a while, then update authorizations.
    //
    // If `immediate` is `true`, fetch without waiting. Otherwise, wait until the daemon responds.
    async _update(options: { immediate?:boolean } = {}) {
        if (this._lock) {
            console.log("keep-it-focused", "ConfigManager", "update", "update already in progress");
            return;
        }

        console.log("keep-it-focused", "ConfigManager", "update", "checking whether we need to update");
        let config;
        try {
            this._lock = this._fetch(options);
            config = await this._lock;
        } finally {
            this._lock = null;
        }

        // We only reach this point if `this._fetch()` didn't raise an error.        
        console.debug("keep-it-focused", "ConfigManager", "update", "we have received an update", config);
        this._processUpdate(config);
    }

    // Process an update *synchronously*, then return a Promise for the flush.
    //
    // We MUST NOT be interrupted while processing it.
    _processUpdate(config: Map<string, Interval[]>): Promise<void> {
        console.debug("keep-it-focused", "ConfigManager", "update", "processing update", config);

        // For each domain, find out if there have been any change.
        let keys: Set<string> = new Set();
        for (let k of config.keys()) {
            keys.add(k);
        }
        for (let k of this._config.keys()) {
            keys.add(k);
        }

        for (let domain of keys) {
            let before = new Set(this._config.get(domain) || []);
            let after = new Set(config.get(domain) || []);
            
            for (let interval of after) {
                if (before.has(interval)) {
                    // Interval was neither added nor removed.
                } else {
                    console.debug("keep-it-focused", "ConfigManager", "update", "we have added an authorization", domain, interval);
                    timeManager.addInterval(domain, interval);
                }
            }
            for (let interval of before) {
                if (after.has(interval)) {
                    // Interval was neither added nor removed.
                } else {
                    console.debug("keep-it-focused", "ConfigManager", "update", "we have removed an authorization", domain, interval);
                    timeManager.removeInterval(domain, interval);
                }
            }
        }
        this._config = config;

        // Don't forget to flush rules!
        return timeManager.flush();
    }

    // Fetch instructions.
    async _fetch(options: {immediate?: boolean}): Promise<Map<string, Interval[]>> {
        let now = Date.now();
        let url;
        if (options.immediate) {
            console.log("keep-it-focused", "ConfigManager", "requesting immediate update");
            url = "http://localhost:7878/immediate";
        } else {
            console.log("keep-it-focused", "ConfigManager", "requesting update upon next server-controlled refresh");
            url = "http://localhost:7878/";
        }
        let response = await fetch(url, {
            method: "GET",
        });
        if (!response.ok) {
            console.error("keep-it-focused", "ConfigManager", "could not get in touch with update server, skipping this update");
            throw new Error("could not get in touch with update server");
        }
        type Payload = { [domain: string]: {start: string, end: string}[] };
        let json = await response.json() as Payload;
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
        this._latestUpdateTS = now;
        return config;
    }
};
// Global instance of the ConfigManager.
const configManager = new ConfigManager();


// A regex for times in HHMM format.
const HHMM = /(\d\d)(\d\d)/;

// Convert a time in HHMM to a date in today (or tomorrow).
//
// Conversions assume that HHMM uses the local time zone.
function hhmmToDate(source: string): Date {
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


// On startup, setup.
browser.runtime.onInstalled.addListener(async () => {
    try {
        console.log("keep-it-focused", "setup", "starting");
        await ruleManager.init();
        console.log("keep-it-focused", "setup", "launching first update");
        await configManager.init();
        console.log("keep-it-focused", "setup", "complete");
    } catch (ex) {
        console.error("keep-it-focused", "setup", "error", ex);
    }
});
// On uninstall, eventually, stop the fetch loop.
browser.runtime.onSuspend.addListener(async () => {
    console.log("keep-it-focused", "suspend", "preparing");
    configManager.suspend();
})


declare namespace console {
    function debug(...messages: any[]): void;
    function info(...messages: any[]): void;
    function log(...messages: any[]): void;
    function error(...messages: any[]): void;
    function warn(...messages: any[]): void;
}

declare namespace browser {
    namespace declarativeNetRequest {
        class Rule {
            id: number
            action: {
                type: "block" // Other types to be added.
            }
            condition: { // Other types to be added.
                urlFilter: string
            }
        }
        function getSessionRules(filter?: { ruleIds?: number[] }): Promise<Rule[]>
        function updateSessionRules(options: {addRules?: declarativeNetRequest.Rule[], removeRuleIds?: number[]}): Promise<void>
    }
    namespace tabs {
        class Tab {
            id?: number
        }
        class ChangeInfo {
            url?: string
        }
        function update(id: number, options: {
            url: string,
            autoDiscardable?: boolean,
        }): Promise<tabs.Tab>
        function query(filter: {
            url: string | string[],
            active?: boolean,
        }): Promise<tabs.Tab[]>
        namespace onUpdated {
            type Callback = (id: number, change: tabs.ChangeInfo, tab: tabs.Tab) => void;
            function addListener(callback: Callback, filter: { urls?: string[], properties?: ["url"] }): void
            function removeListener(callback: Callback): void
        }
    }
    namespace notifications {
        function create(options?: {
            type: "basic",
            title: string,
            message: string,
            iconUrl?: string,
        } | {
            type: "progress",
            title: string,
            message: string,
            iconUrl?: string,
            progress: number,
        }): Promise<void>
    }
    namespace runtime {
        namespace onInstalled {
            type Callback = () => void;
            function addListener(callback: Callback): void
        }
        namespace onSuspend {
            type Callback = () => void;
            function addListener(callback: Callback): void
        }
    }
    namespace alarms {
        type Alarm = {
            name: string,
            scheduledTime: number,
            periodInMinutes?: number,
        }
        function create(id: string, when: {
            when: number,
            periodInMinutes?: number,
        } | {
            delayInMinutes: number,
            periodInMinutes?: number,
        } | {
            periodInMinutes: number,
        }): void
        function clear(id: string): Promise<void>
        namespace onAlarm {
            function addListener(cb: (alarm: Alarm) => void): void
        }
    }
}
declare function fetch(resource: string, options?: FetchOptions): Promise<FetchResponse>;
interface FetchRequest { }
type FetchOptions = {
    method: "GET",
}
type FetchResponse = {
    ok: false
} | {
    ok: true,
    json(): Promise<object>
}