@echo off
REM Compile GLSL shaders to SPIR-V for PrismaRev
REM Requires glslc from Vulkan SDK (https://vulkan.lunarg.com/)

if "%VK_SDK%"=="" set VK_SDK=D:\VulkanSDK\1.4.350.0
set GLSLC=%VK_SDK%\Bin\glslc.exe

if not exist "%GLSLC%" (
    echo ERROR: glslc not found at %GLSLC%
    echo Install Vulkan SDK or set VK_SDK to the correct path.
    exit /b 1
)

echo Compiling shaders...

"%GLSLC%" mesh.vert -o mesh.vert.spv
if %ERRORLEVEL% neq 0 (
    echo FAILED: mesh.vert
    exit /b %ERRORLEVEL%
)
echo   mesh.vert -^> mesh.vert.spv

"%GLSLC%" mesh.frag -o mesh.frag.spv
if %ERRORLEVEL% neq 0 (
    echo FAILED: mesh.frag
    exit /b %ERRORLEVEL%
)
echo   mesh.frag -^> mesh.frag.spv

"%GLSLC%" pbr.frag -o pbr.frag.spv
if %ERRORLEVEL% neq 0 (
    echo FAILED: pbr.frag
    exit /b %ERRORLEVEL%
)
echo   pbr.frag -^> pbr.frag.spv

"%GLSLC%" overlay.vert -o overlay.vert.spv
if %ERRORLEVEL% neq 0 (
    echo FAILED: overlay.vert
    exit /b %ERRORLEVEL%
)
echo   overlay.vert -^> overlay.vert.spv

"%GLSLC%" overlay.frag -o overlay.frag.spv
if %ERRORLEVEL% neq 0 (
    echo FAILED: overlay.frag
    exit /b %ERRORLEVEL%
)
echo   overlay.frag -^> overlay.frag.spv

"%GLSLC%" gizmo.vert -o gizmo.vert.spv
if %ERRORLEVEL% neq 0 (
    echo FAILED: gizmo.vert
    exit /b %ERRORLEVEL%
)
echo   gizmo.vert -^> gizmo.vert.spv

"%GLSLC%" gizmo.frag -o gizmo.frag.spv
if %ERRORLEVEL% neq 0 (
    echo FAILED: gizmo.frag
    exit /b %ERRORLEVEL%
)
echo   gizmo.frag -^> gizmo.frag.spv

echo All shaders compiled successfully.
