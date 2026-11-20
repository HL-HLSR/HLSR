using System; using System.Linq; using Mono.Cecil; using Mono.Cecil.Cil;
// Патч SourceSplit 3.3.4.6 для resources/livesplit-fixes (см.
// src/main/linux/LiveSplitFixes.js). Под wine-mono Process.MainModule у только
// что созданного процесса бросает ArgumentOutOfRangeException (пустой список
// модулей), а SourceSplit пропускает молча только Win32Exception. Вызов в
// предикате TryGetGameProcess меняется на помощник, который возвращает null,
// и SourceSplit повторяет попытку через 750 мс.
//
// Сборка и запуск (Wine, mcs и Mono.Cecil 0.11.1 из wine-mono в Proton):
//   wine <wine-mono>/lib/mono/4.5/mcs.exe -r:Mono.Cecil.dll -out:patch.exe patch.cs
//   wine patch.exe LiveSplit.SourceSplit.dll patched.dll
// Результат детерминирован: sha256 должен совпасть с fixed в LiveSplitFixes.js.
class Patch {
  static int Main(string[] a) {
    var m = ModuleDefinition.ReadModule(a[0]);
    var gm = m.GetType("LiveSplit.SourceSplit.GameHandling.GameMemory");
    var dc = gm.NestedTypes.Single(t => t.Name == "<>c__DisplayClass84_0");
    var lambda = dc.Methods.Single(x => x.Name == "<TryGetGameProcess>b__2");
    var calls = lambda.Body.Instructions.Where(i => i.OpCode == OpCodes.Callvirt && ((MethodReference) i.Operand).FullName == "System.Diagnostics.ProcessModule System.Diagnostics.Process::get_MainModule()").ToList();
    if (calls.Count != 1) { Console.WriteLine("expected 1 MainModule call, got " + calls.Count); return 2; }
    var getter = (MethodReference) calls[0].Operand;

    var helper = new MethodDefinition("HlsrMainModuleOrNull", MethodAttributes.Private | MethodAttributes.Static | MethodAttributes.HideBySig, getter.ReturnType);
    helper.Parameters.Add(new ParameterDefinition("p", ParameterAttributes.None, getter.DeclaringType));
    var result = new VariableDefinition(getter.ReturnType);
    helper.Body.Variables.Add(result);
    helper.Body.InitLocals = true;
    var il = helper.Body.GetILProcessor();
    var end = il.Create(OpCodes.Ldloc, result);
    var tryStart = il.Create(OpCodes.Ldarg_0);
    il.Append(tryStart);
    il.Append(il.Create(OpCodes.Callvirt, getter));
    il.Append(il.Create(OpCodes.Stloc, result));
    il.Append(il.Create(OpCodes.Leave, end));
    var handler = il.Create(OpCodes.Pop);
    il.Append(handler);
    il.Append(il.Create(OpCodes.Ldnull));
    il.Append(il.Create(OpCodes.Stloc, result));
    il.Append(il.Create(OpCodes.Leave, end));
    il.Append(end);
    il.Append(il.Create(OpCodes.Ret));
    var mscorlib = m.AssemblyReferences.First(r => r.Name == "mscorlib");
    var aoore = new TypeReference("System", "ArgumentOutOfRangeException", m, mscorlib);
    helper.Body.ExceptionHandlers.Add(new ExceptionHandler(ExceptionHandlerType.Catch) {
      TryStart = tryStart, TryEnd = handler, HandlerStart = handler, HandlerEnd = end, CatchType = aoore });
    gm.Methods.Add(helper);

    calls[0].OpCode = OpCodes.Call;
    calls[0].Operand = helper;
    m.Write(a[1]);
    Console.WriteLine("patched");
    return 0;
  }
}
