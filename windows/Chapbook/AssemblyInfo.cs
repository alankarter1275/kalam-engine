using System.Runtime.CompilerServices;

// Every type crossing this boundary is blittable, and this is what makes
// that a compile-time claim rather than a hope.
//
// With runtime marshalling on, the CLR silently converts at the boundary:
// a `bool` field becomes a four-byte Win32 `BOOL`, a `char` becomes ANSI,
// and a struct whose layout no longer matches the header still compiles
// and misreads every field after the first mismatch. Turning it off makes
// each of those a build error in `Native.cs` instead — which is why the
// native structs there use `byte` where the header says `bool`, and why
// the conversion happens once, in the public types, rather than invisibly
// at ninety call sites.
[assembly: DisableRuntimeMarshalling]

// The test project reads the `Native*` structs by reflection and compares
// them field for field against the checked-in header. They are internal
// because nothing outside this assembly should name a raw layout, and the
// gate that keeps those layouts honest is the one exception.
[assembly: System.Runtime.CompilerServices.InternalsVisibleTo("Chapbook.Tests")]
