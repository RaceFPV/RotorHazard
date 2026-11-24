import sys, json, dataclasses, types
sys.path.insert(0, 'src/server')

class _StubRhapi:
    def __(self, s): return s
rhapi = _StubRhapi()

from enum import Enum
class SeedMethod(Enum):
    INPUT = 0
    HEAT_INDEX = 1
    CLASS_INDEX = 2
@dataclasses.dataclass
class HeatPlanSlot:
    method: SeedMethod
    seed_rank: int
    seed_index: object = None
@dataclasses.dataclass
class HeatPlan:
    name: str
    slots: list = None

class UIFieldSelectOption:
    def __init__(self, *a, **k): pass
class UIFieldType:
    SELECT='select'; BASIC_INT='int'; TEXT='text'; CHECKBOX='check'
    UIFieldSelectOption = UIFieldSelectOption
class UIField:
    def __init__(self, *a, **k): pass

hgmod = types.ModuleType('HeatGenerator')
hgmod.SeedMethod = SeedMethod
hgmod.HeatPlanSlot = HeatPlanSlot
hgmod.HeatPlan = HeatPlan
class HeatGenerator:
    def __init__(self, *a, **k): pass
hgmod.HeatGenerator = HeatGenerator
sys.modules['HeatGenerator'] = hgmod

rhui_stub = types.ModuleType('RHUI')
rhui_stub.UIField = UIField
rhui_stub.UIFieldType = UIFieldType
rhui_stub.UIFieldSelectOption = UIFieldSelectOption
sys.modules['RHUI'] = rhui_stub
eventmanager = types.ModuleType('eventmanager')
class Evt: HEAT_GENERATOR_INITIALIZE = 'heatGeneratorInitialize'
eventmanager.Evt = Evt
sys.modules['eventmanager'] = eventmanager

import importlib.util
spec = importlib.util.spec_from_file_location('hg', 'src/server/bundled_plugins/rh_heatgenerator_standard/__init__.py')
hg = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hg)

out = {}
for fn_name in ['bracket_1e_16_fai', 'bracket_1e_32_fai', 'bracket_1e_64_fai',
                'bracket_2e_16_fai', 'bracket_2e_32_fai', 'bracket_2e_64_fai',
                'bracket_2e_16_multigp']:
    plans = getattr(hg, fn_name)(rhapi)
    out[fn_name] = [
        {'name': p.name,
         'slots': [{'method': int(s.method.value), 'seed_rank': s.seed_rank,
                    'seed_index': s.seed_index} for s in p.slots]}
        for p in plans
    ]
print(json.dumps(out))
