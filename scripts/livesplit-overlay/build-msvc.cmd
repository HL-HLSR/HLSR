@echo off
rem Временная сборка прокси d3d9.dll и opengl32.dll на Windows: MSVC x86
rem (Visual Studio Build Tools 2022), без CRT, те же .def, что у build.sh.
rem Основная сборка — build.sh на Linux (clang + lld и заголовки Wine): после
rem этой сборки proxy-d3d9.dll и proxy-opengl32.dll надо пересобрать ею.
rem
rem   scripts\livesplit-overlay\build-msvc.cmd [папка для DLL]
rem
rem По умолчанию DLL кладутся в resources\livesplit-overlay. /Brepro: без
rem отметок времени, повторный запуск даёт те же байты; sha256 закоммиченных
rem DLL — в proxy.sha256 рядом, после пересборки его обновить.
setlocal
set "here=%~dp0"
set "out=%~1"
if "%out%"=="" set "out=%here%..\..\resources\livesplit-overlay"
set "vcvars=%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvarsall.bat"
call "%vcvars%" x86 >nul || exit /b 1
set "work=%TEMP%\hlsr-overlay-msvc-%RANDOM%%RANDOM%"
mkdir "%work%" || exit /b 1
set cflags=/nologo /c /O2 /GS- /Zl /utf-8 /W3 /Brepro /D_WIN32_WINNT=0x0601 /DWINVER=0x0601
set ldflags=/nologo /DLL /NODEFAULTLIB /ENTRY:DllMain /SUBSYSTEM:WINDOWS /MACHINE:X86 /DYNAMICBASE /NXCOMPAT /OPT:REF /OPT:ICF /Brepro
rem В .def имена __stdcall записаны так, как их называет clang (без «_»).
set aliases=/ALTERNATENAME:hook_Direct3DCreate9@4=_hook_Direct3DCreate9@4 /ALTERNATENAME:hook_Direct3DCreate9Ex@8=_hook_Direct3DCreate9Ex@8
cl %cflags% /Fo"%work%\proxy-d3d9.obj" "%here%proxy-d3d9.c" || goto fail
link %ldflags% %aliases% /DEF:"%here%proxy-d3d9.def" /OUT:"%work%\d3d9.dll" "%work%\proxy-d3d9.obj" kernel32.lib user32.lib gdi32.lib || goto fail
rem opengl32.def — как в build.sh: переходник на каждую функцию из списка и
rem перехват wglSwapBuffers. Построчно: скобки в «X(» ломают блок ( … ) cmd.
set "def=%work%\opengl32.def"
> "%def%" echo LIBRARY opengl32.dll
>> "%def%" echo EXPORTS
for /f "tokens=2 delims=()" %%n in ('findstr /b "X(" "%here%proxy-gl-exports.inc"') do >> "%def%" echo   %%n = thunk_%%n
>> "%def%" echo   wglSwapBuffers = hook_wglSwapBuffers@4
cl %cflags% /Fo"%work%\proxy-gl.obj" "%here%proxy-gl.c" || goto fail
link %ldflags% /ALTERNATENAME:hook_wglSwapBuffers@4=_hook_wglSwapBuffers@4 /DEF:"%work%\opengl32.def" /OUT:"%work%\opengl32.dll" "%work%\proxy-gl.obj" kernel32.lib user32.lib gdi32.lib || goto fail
copy /y "%work%\d3d9.dll" "%out%\proxy-d3d9.dll" >nul || goto fail
copy /y "%work%\opengl32.dll" "%out%\proxy-opengl32.dll" >nul || goto fail
rmdir /s /q "%work%"
certutil -hashfile "%out%\proxy-d3d9.dll" SHA256 | findstr /v ":"
certutil -hashfile "%out%\proxy-opengl32.dll" SHA256 | findstr /v ":"
exit /b 0
:fail
rmdir /s /q "%work%"
exit /b 1
