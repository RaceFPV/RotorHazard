# Generated from src/server/RHUI.py by rust/tools/gen_rh_compat.py.
# Ships with the plugin bridge so plugins can import server internals on a
# machine that has no Python server. Values are sliced, not retyped.


from enum import Enum
from typing import Any, Optional, Dict, List, Callable
from dataclasses import dataclass, field

class UIFieldType(Enum):
    TEXT = "text"
    BASIC_INT = "basic_int"
    NUMBER = "number"
    RANGE = "range"
    SELECT = "select"
    CHECKBOX = "checkbox"
    PASSWORD = "password"
    DATE = "date"
    TIME = "time"
    DATETIME = "datetime"
    EMAIL = "email"
    TEL = "tel"
    URL = "url"
    STATUS = "status"

@dataclass
class UIFieldSelectOption():
    value: str
    label: str

@dataclass
class UIField():
    name: str
    label: str
    field_type: str = UIFieldType.TEXT
    value: Any = None
    desc: str = None
    placeholder: str = None
    options: List[UIFieldSelectOption] = None
    order: int = 0 # not implemented
    private: bool = False
    html_attributes: dict = None
    persistent_section: str = None
    persistent_restart: bool = False

    def frontend_repr(self):
        return {
            'name': self.name,
            'label': self.label,
            'field_type': self.field_type.value,
            'value': self.value,
            'desc' : self.desc,
            'placeholder': self.placeholder,
            'options': [asdict(option) for option in self.options] if self.options else None,
            'order': self.order,
            'html_attributes': self.html_attributes,
            'section': self.persistent_section
        }
