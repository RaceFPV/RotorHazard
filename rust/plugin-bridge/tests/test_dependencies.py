"""Dependency handling in the plugin bridge: no server, no network.

The interesting cases are the ones that would otherwise look like a plugin bug at
runtime: a dependency that is already there, one that installs, one that fails to
install, and a machine with auto-install turned off.
"""
import importlib.util
import os
import sys
import types

HERE = os.path.dirname(os.path.abspath(__file__))
BRIDGE = os.path.abspath(os.path.join(HERE, '..', 'rhapi_bridge.py'))

if importlib.util.find_spec('socketio') is None:  # only needed for the import
    sys.modules.setdefault('socketio', types.ModuleType('socketio'))

spec = importlib.util.spec_from_file_location('rh_bridge', BRIDGE)
bridge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bridge)

failures = []


def check(name, condition, detail=''):
    if condition:
        print('  ok   %s' % name)
    else:
        failures.append('%s %s' % (name, detail))
        print('  FAIL %s %s' % (name, detail))


def new_bridge():
    cls = next(obj for obj in vars(bridge).values()
               if isinstance(obj, type) and hasattr(obj, 'load_plugins'))
    return object.__new__(cls)


class Result:
    def __init__(self, code=0, stderr='', stdout=''):
        self.returncode = code
        self.stderr = stderr
        self.stdout = stdout


print('no dependencies')
b = new_bridge()
called = []
b._pip_install = lambda specs: called.append(specs) or Result()
check('empty manifest is satisfied',
      b._ensure_dependencies({}) is None and not called)
check('manifest without the key is satisfied',
      b._ensure_dependencies({'name': 'x'}) is None and not called)

print('satisfied dependency')
b = new_bridge()
b._pip_install = lambda specs: called.append(specs) or Result()
b._missing_specs = lambda specs: []
check('installed dependency needs no pip call',
      b._ensure_dependencies({'dependencies': ['socketio>=5']}) is None and not called)

print('missing dependency, install works')
b = new_bridge()
calls = []
b._pip_install = lambda specs: calls.append(list(specs)) or Result()
state = {'first': True}
b._missing_specs = (lambda specs: ['esptool>=4.8.1']
                    if state.pop('first', False) else [])
check('installs when missing', b._ensure_dependencies(
    {'dependencies': ['esptool>=4.8.1']}) is None)
check('install used the declared specifier', calls == [['esptool>=4.8.1']], calls)

print('missing dependency, install fails')
b = new_bridge()
b._pip_install = lambda specs: Result(1, stderr='ERROR: No matching distribution found')
b._missing_specs = lambda specs: ['esptool>=4.8.1']
problem = b._ensure_dependencies({'dependencies': ['esptool>=4.8.1']})
check('failure is reported, not raised', isinstance(problem, str), repr(problem))
check('message quotes pip and the command',
      'failed to install' in problem and '-m pip install' in problem, problem)

print('missing dependency, still missing after install')
b = new_bridge()
b._pip_install = lambda specs: Result()
b._missing_specs = lambda specs: ['esptool>=4.8.1']
problem = b._ensure_dependencies({'dependencies': ['esptool>=4.8.1']})
check('silent no-op install is reported',
      'not importable' in problem, problem)

print('auto-install disabled')
os.environ['RH_PLUGIN_NO_DEPS'] = '1'
try:
    b = new_bridge()
    called = []
    b._pip_install = lambda specs: called.append(specs) or Result()
    b._missing_specs = lambda specs: ['esptool>=4.8.1']
    problem = b._ensure_dependencies({'dependencies': ['esptool>=4.8.1']})
    check('reports the command instead of installing',
          'auto-install disabled' in problem and not called, repr(problem))
finally:
    del os.environ['RH_PLUGIN_NO_DEPS']

print('specifier parsing')
b = new_bridge()
check('extras and markers are stripped',
      b._dependency_name('pyserial[cp2110]>=3.5; python_version>"3"') == 'pyserial',
      b._dependency_name('pyserial[cp2110]>=3.5; python_version>"3"'))

if failures:
    print('FAIL')
    for failure in failures:
        print('  -', failure)
    sys.exit(1)
print('PASS: plugin dependency handling')
