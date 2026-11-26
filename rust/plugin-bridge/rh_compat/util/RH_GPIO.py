"""Stub: the Rust server has no GPIO backend; RHUtils only calls these
from hardware-specific helpers."""


def check_input_tied_low(pin):
    return False


def is_real_hw_GPIO():
    return False


def get_GPIO_type_str():
    return 'none'
