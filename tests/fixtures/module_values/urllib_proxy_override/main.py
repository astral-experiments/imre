from urllib.request import _proxy_bypass_winreg_override

override = "proxy.example; *.internal.test; <local>"
assert _proxy_bypass_winreg_override("proxy.example:8080", override)
assert _proxy_bypass_winreg_override("a.internal.test:443", override)
assert _proxy_bypass_winreg_override("printer:80", override)
assert not _proxy_bypass_winreg_override("internal.test", override)
assert not _proxy_bypass_winreg_override("remote.example", override)
assert not _proxy_bypass_winreg_override("printer", "")
