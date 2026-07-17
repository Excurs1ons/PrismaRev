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
set PROFILE=sspirv_1_5
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
