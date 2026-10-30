# AppImage runtime and library notices

This AppImage uses **libfuse 3.15.0 under LGPL-2.1**, with its use covered by that licence. Its original licence, authors and complete LGPL terms are included here. The matching library source, runtime source, patches and build recipes needed to modify and relink the runtime are supplied in `notdot-corresponding-source.tar.gz` beside the AppImage release download.

The AppRun launcher and type-2 runtime use the MIT licence. The runtime also includes squashfuse 0.5.2 (BSD-2-Clause), musl 1.2.5, mimalloc 2.1.7, zlib 1.3.2 and zstd 1.5.6; their original notices and terms are retained in this directory. Original source-header credits supplement the repository-level licences.

Other bundled distro-library notices are under `../platform-copyright/`. The selected components, versions, exact source archives and file hashes are identified in the corresponding-source manifest. That directory conservatively retains build-environment package notices; its presence alone does not mean every package's software is shipped.

notdot's own source remains Apache-2.0. These libraries and launchers retain their own licences.
