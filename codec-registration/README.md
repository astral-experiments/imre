# User codec registry and the intact CTextIOWrapperTest

The stock reference is Linux CPython 3.14.5 at 5607950ef232dad16d75c0cf53101d9649d89115. The source-pinned native public baseline 36c4f334e96c3bb5a8270ff8f8c85808715fec6d appears in the earlier buffered-rw-pair class packet. It registered all 82, then errored on the missing codecs.register during every setUp.

Development revision 9d54510b5601917481fc2a6c228cdccd2e11eba0 adds the first codec search path on public parent a23a577bcd59a7322af266ff9ec4e8f569f2f929. Its focused archive preserves the 3.14.5 reference and both native optimization levels. It separates the accepted four-tuple path, callback lifetime and cache behavior from the enabled original lookupTestDecoder callback, which still raised AttributeError on the missing CodecInfo constructor at that revision. Supplemental controls do not count as original class cases.
