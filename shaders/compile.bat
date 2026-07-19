@echo off
REM Compile PrismaRev Slang shaders to SPIR-V + emit reflection JSON.
REM Mirrors shaders/compile.sh for the Windows desktop toolchain.
REM
REM Requires `slangc` on PATH (from a Slang release, e.g. tools\slang\bin\slangc.exe),
REM or set SLANGC to the full path. This script is intended for DESKTOP / CI hosts,
REM NOT for Termux / Android devices (slangc is glibc/MSVC only). The engine
REM ships the pre-compiled .spv produced here to mobile.
REM
REM Output per stage:
REM   shaders\<name>.<stage>.spv       - SPIR-V (vert/frag/comp) included by the engine
REM   shaders\reflection\<name>.json   - slang reflection (drives xtask shader-bindgen)

setlocal enableextensions enabledelayedexpansion

set HERE=%~dp0
pushd "%HERE%"

set SRC=slang
set REFL=reflection
set PROFILE=spirv_1_5
if not "%SLANG_PROFILE%"=="" set PROFILE=%SLANG_PROFILE%

if "%SLANGC%"=="" set SLANGC=slangc
where %SLANGC% >nul 2>&1
if errorlevel 1 (
    if not exist "%SLANGC%" (
        echo ERROR: slangc not found. Install a Slang release or set SLANGC=^<path^>\slangc.exe
        exit /b 1
    )
)

if not exist "%REFL%" mkdir "%REFL%"

echo Compiling Slang shaders (slangc = %SLANGC%, profile = %PROFILE%)...

REM mesh: vertex + fragment
%SLANGC% "%SRC%\mesh.slang"   -profile %PROFILE% -target spirv -entry vertexMain   -stage vertex   -fvk-use-entrypoint-name -o mesh.vert.spv   || goto :fail
echo   mesh :: vertexMain   -^> mesh.vert.spv
%SLANGC% "%SRC%\mesh.slang"   -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o mesh.frag.spv   || goto :fail
echo   mesh :: fragmentMain -^> mesh.frag.spv
%SLANGC% "%SRC%\mesh.slang"   -profile %PROFILE% -target spirv -entry vertexMain -stage vertex -entry fragmentMain -stage fragment -reflection-json "%REFL%\mesh.json" -o "%REFL%\mesh.tmp.spv"   || goto :fail
del /q "%REFL%\mesh.tmp.spv" 2>nul
echo   reflect mesh -^> reflection\mesh.json

REM pbr: fragment only (reuses mesh.slang vertex stage at pipeline level)
%SLANGC% "%SRC%\pbr.slang"    -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o pbr.frag.spv   || goto :fail
echo   pbr :: fragmentMain -^> pbr.frag.spv
%SLANGC% "%SRC%\pbr.slang"    -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -reflection-json "%REFL%\pbr.json" -o "%REFL%\pbr.tmp.spv"   || goto :fail
del /q "%REFL%\pbr.tmp.spv" 2>nul
echo   reflect pbr -^> reflection\pbr.json

REM gizmo: vertex + fragment
%SLANGC% "%SRC%\gizmo.slang"  -profile %PROFILE% -target spirv -entry vertexMain   -stage vertex   -fvk-use-entrypoint-name -o gizmo.vert.spv  || goto :fail
echo   gizmo :: vertexMain   -^> gizmo.vert.spv
%SLANGC% "%SRC%\gizmo.slang"  -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o gizmo.frag.spv  || goto :fail
echo   gizmo :: fragmentMain -^> gizmo.frag.spv
%SLANGC% "%SRC%\gizmo.slang"  -profile %PROFILE% -target spirv -entry vertexMain -stage vertex -entry fragmentMain -stage fragment -reflection-json "%REFL%\gizmo.json" -o "%REFL%\gizmo.tmp.spv" || goto :fail
del /q "%REFL%\gizmo.tmp.spv" 2>nul
echo   reflect gizmo -^> reflection\gizmo.json

REM overlay: vertex + fragment
%SLANGC% "%SRC%\overlay.slang" -profile %PROFILE% -target spirv -entry vertexMain   -stage vertex   -fvk-use-entrypoint-name -o overlay.vert.spv  || goto :fail
echo   overlay :: vertexMain   -^> overlay.vert.spv
%SLANGC% "%SRC%\overlay.slang" -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o overlay.frag.spv  || goto :fail
echo   overlay :: fragmentMain -^> overlay.frag.spv
%SLANGC% "%SRC%\overlay.slang" -profile %PROFILE% -target spirv -entry vertexMain -stage vertex -entry fragmentMain -stage fragment -reflection-json "%REFL%\overlay.json" -o "%REFL%\overlay.tmp.spv" || goto :fail
del /q "%REFL%\overlay.tmp.spv" 2>nul
echo   reflect overlay -^> reflection\overlay.json

REM shadow: compute (RayQuery inline shadow pass, half-res)
%SLANGC% "%SRC%\shadow.slang"       -profile %PROFILE% -target spirv -entry computeMain -stage compute  -fvk-use-entrypoint-name -o shadow.comp.spv   || goto :fail
echo   shadow :: computeMain -^> shadow.comp.spv
%SLANGC% "%SRC%\shadow.slang"       -profile %PROFILE% -target spirv -entry computeMain -stage compute  -reflection-json "%REFL%\shadow.json" -o "%REFL%\shadow.tmp.spv"   || goto :fail
del /q "%REFL%\shadow.tmp.spv" 2>nul
echo   reflect shadow -^> reflection\shadow.json

REM shadowmap: vertex + fragment (rasterized depth-only shadow map fallback)
%SLANGC% "%SRC%\shadowmap.slang"  -profile %PROFILE% -target spirv -entry vertexMain   -stage vertex   -fvk-use-entrypoint-name -o shadowmap.vert.spv  || goto :fail
echo   shadowmap :: vertexMain   -^> shadowmap.vert.spv
%SLANGC% "%SRC%\shadowmap.slang"  -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o shadowmap.frag.spv  || goto :fail
echo   shadowmap :: fragmentMain -^> shadowmap.frag.spv

REM scene: vertex + fragment (forward PBR + IBL RenderGraph path)
%SLANGC% "%SRC%\scene.vert.slang" -profile %PROFILE% -target spirv -entry vertexMain   -stage vertex   -fvk-use-entrypoint-name -o scene.vert.spv   || goto :fail
echo   scene :: vertexMain   -^> scene.vert.spv
%SLANGC% "%SRC%\scene.frag.slang" -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o scene.frag.spv   || goto :fail
echo   scene :: fragmentMain -^> scene.frag.spv

REM skybox: vertex + fragment (environment cubemap background)
%SLANGC% "%SRC%\skybox.slang" -profile %PROFILE% -target spirv -entry vertexMain   -stage vertex   -fvk-use-entrypoint-name -o skybox.vert.spv  || goto :fail
echo   skybox :: vertexMain   -^> skybox.vert.spv
%SLANGC% "%SRC%\skybox.slang" -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o skybox.frag.spv  || goto :fail
echo   skybox :: fragmentMain -^> skybox.frag.spv
%SLANGC% "%SRC%\skybox.slang" -profile %PROFILE% -target spirv -entry vertexMain -stage vertex -entry fragmentMain -stage fragment -reflection-json "%REFL%\skybox.json" -o "%REFL%\skybox.tmp.spv"   || goto :fail
del /q "%REFL%\skybox.tmp.spv" 2>nul
echo   reflect skybox -^> reflection\skybox.json

REM sharc_query: compute (SHARC GI cache lookup, half-res)
%SLANGC% "%SRC%\sharc_query.slang"  -profile %PROFILE% -target spirv -entry computeMain -stage compute  -I "%SRC%" -fvk-use-entrypoint-name -o sharc_query.comp.spv   || goto :fail
echo   sharc_query :: computeMain -^> sharc_query.comp.spv
%SLANGC% "%SRC%\sharc_query.slang"  -profile %PROFILE% -target spirv -entry computeMain -stage compute  -I "%SRC%" -reflection-json "%REFL%\sharc_query.json" -o "%REFL%\sharc_query.tmp.spv"   || goto :fail
del /q "%REFL%\sharc_query.tmp.spv" 2>nul
echo   reflect sharc_query -^> reflection\sharc_query.json

REM lighting: fragment (GBuffer + shadow + GI + IBL -^> HDR)
%SLANGC% "%SRC%\lighting.slang"     -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o lighting.frag.spv   || goto :fail
echo   lighting :: fragmentMain -^> lighting.frag.spv
%SLANGC% "%SRC%\lighting.slang"     -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -reflection-json "%REFL%\lighting.json" -o "%REFL%\lighting.tmp.spv"   || goto :fail
del /q "%REFL%\lighting.tmp.spv" 2>nul
echo   reflect lighting -^> reflection\lighting.json

REM post: fragment (ACES tone map -^> swapchain)
%SLANGC% "%SRC%\post.slang"         -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o post.frag.spv   || goto :fail
echo   post :: fragmentMain -^> post.frag.spv
%SLANGC% "%SRC%\post.slang"         -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -reflection-json "%REFL%\post.json" -o "%REFL%\post.tmp.spv"   || goto :fail
del /q "%REFL%\post.tmp.spv" 2>nul
echo   reflect post -^> reflection\post.json

REM bindless: fragment only (pairs with mesh.vert.spv from mesh.slang vertex
REM at pipeline-build time to form the bindless PBR draw pipeline).
%SLANGC% "%SRC%\bindless.slang"  -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o bindless.frag.spv   || goto :fail
echo   bindless :: fragmentMain -^> bindless.frag.spv
REM Slang emits an illegal ArrayStride decoration on the bindless image/sampler
REM runtime arrays; strip it (see fix_spirv.py) so the module validates.
if exist "%REFL%\..\fix_spirv.py" python3 "%REFL%\..\fix_spirv.py" bindless.frag.spv bindless.frag.spv
%SLANGC% "%SRC%\bindless.slang"  -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -reflection-json "%REFL%\bindless.json" -o "%REFL%\bindless.tmp.spv"   || goto :fail
del /q "%REFL%\bindless.tmp.spv" 2>nul
echo   reflect bindless -^> reflection\bindless.json

REM scene_bindless: fragment only (RenderGraph ScenePass bindless PBR +
REM rasterized shadow map). Pairs with mesh.vert.spv. This is the
REM graph-path counterpart of bindless.slang with shadow-map sampling;
REM lightViewProj is read from the per-frame UBO (not push constants) so
REM the push constant stays under the 128-byte Vulkan limit.
%SLANGC% "%SRC%\scene_bindless.slang" -profile %PROFILE% -target spirv -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o scene_bindless.frag.spv   || goto :fail
echo   scene_bindless :: fragmentMain -^> scene_bindless.frag.spv
if exist "%REFL%\..\fix_spirv.py" python3 "%REFL%\..\fix_spirv.py" scene_bindless.frag.spv scene_bindless.frag.spv

echo.
echo All Slang shaders compiled successfully.
popd
endlocal
exit /b 0

:fail
echo.
echo Slang shader compilation FAILED.
popd
endlocal
exit /b 1
