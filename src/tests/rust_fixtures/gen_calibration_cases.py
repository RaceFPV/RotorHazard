import calibration, random, json, os
random.seed(42)
cases = []
def trace(passes_at, base=40, amp=90, noise=3, rate=100, dur=60):
    n = int(dur*rate)
    vals = [base + int(random.gauss(0, noise)) for _ in range(n)]
    times = [i/rate for i in range(n)]
    for start, width_s, peak in passes_at:
        i0 = int(start*rate); i1 = i0 + int(width_s*rate)
        for i in range(i0, min(i1, n)):
            frac = (i - i0)/max(1, (i1-i0))
            shape = 1.0 - abs(frac-0.5)*2
            vals[i] = max(vals[i], int(base + amp*shape*shape + random.gauss(0,2)))
    return vals, times
p = [(6+i*8, 0.5, 140+random.randint(-5,5)) for i in range(10)]
v,t = trace(p); cases.append(dict(name='clean', values=v, times=t, start_time=0.0, unlimited_time=False, race_time_sec=60, min_lap_ms=5000, min_lap_behavior=1, min_first_crossing_ms=2000, lap_limit=0))
p = [(3+i*7, 0.4, 130) for i in range(12)] + [(3+12*7+2, 6.0, 120)]
v,t = trace(p); cases.append(dict(name='unlimited_landing', values=v, times=t, start_time=0.0, unlimited_time=True, race_time_sec=0, min_lap_ms=5000, min_lap_behavior=1, min_first_crossing_ms=2000, lap_limit=0))
v = [40+random.randint(-2,2) for _ in range(500)]; t=[i/10 for i in range(500)]
cases.append(dict(name='weak', values=v, times=t, start_time=0.0, unlimited_time=False, race_time_sec=50, min_lap_ms=5000, min_lap_behavior=1, min_first_crossing_ms=2000, lap_limit=0))
p = [(5+i*6, 0.4, 135) for i in range(10)]
v,t = trace(p); cases.append(dict(name='minlap_too_long', values=v, times=t, start_time=0.0, unlimited_time=False, race_time_sec=70, min_lap_ms=12000, min_lap_behavior=1, min_first_crossing_ms=2000, lap_limit=0))
p = [(2, 1.2, 105)] + [(10+i*8, 0.5, 145) for i in range(9)]
v,t = trace(p); cases.append(dict(name='holeshot', values=v, times=t, start_time=0.0, unlimited_time=False, race_time_sec=90, min_lap_ms=5000, min_lap_behavior=1, min_first_crossing_ms=2000, lap_limit=0))
p = [(4+i*7, 0.4, 140) for i in range(9)]
v,t = trace(p); cases.append(dict(name='lap_limit', values=v, times=t, start_time=0.0, unlimited_time=False, race_time_sec=70, min_lap_ms=5000, min_lap_behavior=1, min_first_crossing_ms=2000, lap_limit=6))
p = [(2+i*1.5, 0.3, 90+random.randint(-20,20)) for i in range(30)]
v,t = trace(p, amp=60); cases.append(dict(name='noisy', values=v, times=t, start_time=0.0, unlimited_time=False, race_time_sec=50, min_lap_ms=5000, min_lap_behavior=1, min_first_crossing_ms=2000, lap_limit=0))
p = [(-1, 0.4, 140), (6, 0.5, 150), (14, 0.5, 145), (22, 0.5, 148)]
v,t = trace(p, dur=30); cases.append(dict(name='early_start', values=v, times=t, start_time=2.0, unlimited_time=False, race_time_sec=30, min_lap_ms=5000, min_lap_behavior=1, min_first_crossing_ms=2000, lap_limit=0))
out = []
for c in cases:
    res, reason = calibration.suggest_calibration_with_reason(
        c['values'], c['times'], c['start_time'], c['unlimited_time'], c['race_time_sec'],
        c['min_lap_ms'], c['min_lap_behavior'], c['min_first_crossing_ms'], c['lap_limit'])
    out.append(dict(name=c['name'], values=c['values'], times=c['times'],
                    params={k:c[k] for k in ('start_time','unlimited_time','race_time_sec','min_lap_ms','min_lap_behavior','min_first_crossing_ms','lap_limit')},
                    expected=dict(enter_at=res['enter_at'] if res else None, exit_at=res['exit_at'] if res else None,
                                  lap_count=res['lap_count'] if res else None,
                                  band=res['band'] if res else None,
                                  reason=reason)))
    print(c['name'], '->', out[-1]['expected'])
os.makedirs('rust/server/tests/fixtures', exist_ok=True)
json.dump(out, open('rust/server/tests/fixtures/calibration_cases.json','w'))
print('cases written:', len(out))
