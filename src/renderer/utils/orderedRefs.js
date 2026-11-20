// Во Vue 3 ref внутри v-for даёт массив без гарантии порядка. Навигация
// с клавиатуры берёт элементы по индексу, поэтому упорядочиваем их так,
// как они стоят в документе.
export default function orderedRefs(refs) {
  if (!refs) return [];
  const list = Array.isArray(refs) ? refs.filter(Boolean) : [refs];
  return list.sort((a, b) =>
    a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1
  );
}
