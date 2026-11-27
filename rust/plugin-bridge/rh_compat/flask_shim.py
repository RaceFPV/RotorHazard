"""A very small `flask` stand-in, used only when flask is not installed.

Community plugins commonly do `from flask import Blueprint` at module level so
they can add HTTP routes to the server. This server does not serve plugin
blueprints yet, so the blueprint records what was registered and routes nothing:
the plugin still loads, and its panels and buttons work. If a real flask is
importable the bridge leaves it alone.
"""


class Blueprint:
    def __init__(self, name, import_name, **kwargs):
        self.name = name
        self.import_name = import_name
        self.options = kwargs
        self.routes = []

    def route(self, rule, **options):
        def decorator(function):
            self.routes.append((rule, options, function))
            return function

        return decorator

    # flask's method helpers are decorators with the same shape
    get = post = put = patch = delete = route

    def add_url_rule(self, rule, endpoint=None, view_func=None, **options):
        self.routes.append((rule, options, view_func))

    def before_request(self, function):
        return function

    def after_request(self, function):
        return function


class _Request:
    """Enough of flask.request that a module-level import and attribute read work."""

    args = {}
    form = {}
    json = {}
    data = b''
    headers = {}
    values = {}
    remote_addr = '127.0.0.1'

    def get_json(self, *args, **kwargs):
        return {}


request = _Request()


def jsonify(*args, **kwargs):
    if len(args) == 1 and not kwargs:
        return args[0]
    return args or kwargs


def render_template(template, **context):
    return ''


def render_template_string(source, **context):
    return ''


def url_for(endpoint, **values):
    return ''


def redirect(location, code=302):
    return location


def abort(code, *args, **kwargs):
    raise RuntimeError('abort(%s)' % code)


def flash(*args, **kwargs):
    return None


def get_flashed_messages(*args, **kwargs):
    return []


class Response:
    def __init__(self, response=None, status=200, headers=None):
        self.response = response
        self.status_code = status
        self.headers = headers or {}


def make_response(*args, **kwargs):
    return Response(*args, **kwargs)
