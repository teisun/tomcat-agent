// pi_ms_shim.js — `ms` npm package stub for WasmEdge QuickJS.
// Parses human-readable time strings to milliseconds and vice versa.
(function () {
  'use strict';

  var UNITS = {
    ms: 1, msec: 1, msecs: 1,
    s: 1000, sec: 1000, secs: 1000, second: 1000, seconds: 1000,
    m: 60000, min: 60000, mins: 60000, minute: 60000, minutes: 60000,
    h: 3600000, hr: 3600000, hrs: 3600000, hour: 3600000, hours: 3600000,
    d: 86400000, day: 86400000, days: 86400000,
    w: 604800000, week: 604800000, weeks: 604800000,
    y: 31557600000, yr: 31557600000, yrs: 31557600000, year: 31557600000, years: 31557600000
  };

  function ms(val) {
    if (typeof val === 'number') {
      if (val >= 86400000) return Math.round(val / 86400000) + 'd';
      if (val >= 3600000) return Math.round(val / 3600000) + 'h';
      if (val >= 60000) return Math.round(val / 60000) + 'm';
      if (val >= 1000) return Math.round(val / 1000) + 's';
      return val + 'ms';
    }
    var str = String(val || '').trim().toLowerCase();
    var match = str.match(/^(-?\d*\.?\d+)\s*(ms|msecs?|s|secs?|seconds?|m|mins?|minutes?|h|hrs?|hours?|d|days?|w|weeks?|y|yrs?|years?)$/i);
    if (!match) return NaN;
    var num = parseFloat(match[1]);
    var unit = match[2].toLowerCase();
    var mult = UNITS[unit];
    return mult ? num * mult : NaN;
  }

  globalThis.__pi_ms = ms;
  globalThis.__pi_ms["default"] = ms;
})();
