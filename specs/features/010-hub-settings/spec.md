# F010 — Hub, histórico, configurações, onboarding e bandeja

**Status**: Draft · **Release**: v1 (histórico, reuniões, dicionário, modelos, configurações, onboarding, bandeja) · v1.1+ (notas, snippets, estilos)

## Contexto

O Hub é a janela principal: onde o usuário vê o que ditou, gerencia notas e reuniões e configura tudo. Deve ser rápido, navegável por teclado e bilíngue (pt-BR/en).

## Estrutura do Hub (barra lateral)

| Seção                    | Release                      | Conteúdo                              |
| ------------------------ | ---------------------------- | ------------------------------------- |
| **Início**               | v1                           | Histórico de ditados + estatísticas   |
| **Notas**                | v1.1+                        | Scratchpad (F007)                     |
| **Reuniões**             | v1                           | Lista e detalhes (F009)               |
| **Dicionário**           | v1 (vocab + muletas) / v1.1+ | Termos, muletas e substituições       |
| **Snippets**             | v1.1+                        | Gatilhos e expansões                  |
| **Estilos**              | v1.1+                        | Perfis de app, nível de limpeza       |
| **Modelos & Provedores** | v1 (só modelos locais)       | F003; provedores em nuvem na v1.1+    |
| **Configurações**        | v1                           | Geral, Sistema, Privacidade, Avançado |

## Requisitos funcionais

### Início / Histórico

- **FR-010-01** Lista cronológica (agrupada por dia) com: hora, ícone/nome do app, texto final (2 linhas), modo (ditado/comando/nota), status (ícone para falha/copiado).
- **FR-010-02** Busca full-text; filtros por app, modo, status, período.
- **FR-010-03** Detalhe da entrada: texto cru × final (diff), provedor/modelo, idioma, latências (STT/LLM/inserção), estágios do pipeline aplicados.
- **FR-010-04** Ações: copiar, reinserir no app em foco, **tentar novamente** (se o áudio ainda existir, escolhendo o provedor), adicionar palavra ao dicionário a partir de um trecho selecionado, sinalizar, excluir.
- **FR-010-05** Estatísticas: palavras ditadas (hoje/semana/total), velocidade média (palavras por minuto de fala), tempo economizado estimado (vs. 40 ppm digitando), minutos e custo estimado por provedor (F003).
- **FR-010-06** Navegação por teclado (↑/↓ entre entradas, Enter abre, Ctrl+C copia) e compatível com leitor de tela.

### Configurações

- **FR-010-07** **Geral**: atalhos (captura + conflitos, F002), microfone (com medidor de nível ao vivo e teste "grave 3 s e ouça"), idiomas de ditado, idioma da interface, sons.
- **FR-010-08** **Sistema**: iniciar com o Windows (padrão ligado), visibilidade e comportamento da Flow Bar (F001), ícone na bandeja, posição dos toasts.
- **FR-010-09** **Privacidade** (F011): modo offline, retenção de áudio e histórico, enviar título da janela ao LLM, "Apagar todos os dados".
- **FR-010-10** **Avançado**: atrasos de colagem, método de inserção padrão, pré-carregar modelo, descarregar modelo após N min, modo debug de logs, abrir pasta de dados.
- **FR-010-11** P2 — exportar/importar configurações, dicionário, snippets e perfis (JSON, **sem segredos**).
- **FR-010-12** Mudanças aplicadas imediatamente (sem "Salvar"), com validação inline.

### Bandeja

- **FR-010-13** Ícone com estados: normal, gravando (vermelho), erro (âmbar), offline (indicador).
- **FR-010-14** Menu: Abrir Hub · Iniciar/Parar ditado · Iniciar/Parar reunião · Mostrar/Ocultar Flow Bar · Pausar detecção de reuniões por 1 h · Modo offline · Sair. (v1.1+: "Nova nota por voz", F007.)
- **FR-010-15** Fechar o Hub minimiza para a bandeja (o app continua rodando); "Sair" encerra de fato (confirmando se houver gravação ativa).

### Onboarding (primeira execução)

- **FR-010-16** Passos:
  1. Boas-vindas + idioma da interface e de ditado.
  2. **Microfone**: verifica permissão do Windows ("Permitir que apps da área de trabalho acessem o microfone"); se negada, botão que abre `ms-settings:privacy-microphone`; seleção de dispositivo com medidor.
  3. **Transcrição**: escolha do **modelo local** — `large-v3-turbo` marcado como recomendado, rótulos de adequação por hardware (F003), baixa com progresso e dá para seguir enquanto baixa. (v1.1+: opção _Nuvem_ com chave própria.)
  4. **Atalho**: mostra o padrão, permite trocar, e um campo de prática: "Segure `Ctrl+Win` e diga: _Olá, estou testando o Transcreve.ai_" — sucesso quando o texto aparece no campo.
  5. **Flow Bar**: animação mostrando hover e as 2 ações.
  6. **Reuniões**: liga/desliga detecção, explica consentimento.
- **FR-010-17** Onboarding pode ser pulado e reaberto em Configurações → Ajuda.
- **FR-010-18** Instância única: abrir o app de novo foca o Hub existente.
- **FR-010-19** (v1.1+, com a T-049) Atualizações: checagem diária (desativável), notas da versão, instalar ao sair. Na v1 não há updater.

## Requisitos não funcionais

- **NFR-010-01** Hub abre em ≤ 500 ms (janela já criada e oculta).
- **NFR-010-02** Histórico com 100.000 entradas: rolagem fluida (lista virtualizada) e busca ≤ 150 ms.
- **NFR-010-03** Todas as strings via i18n (pt-BR, en), mantidas com a skill `i18n-sync`.
- **NFR-010-04** Acessibilidade conforme as skills `accessibility`/`frontend-a11y` e checagens axe de `rules/react/testing.md`.
- **NFR-010-05** O visual do Hub segue a direção de design da T-008 (`rules/web/design-quality.md` — em particular, evitar o layout genérico "sidebar + cards"), mantendo a organização de seções do Wispr.

## Critérios de aceitação

- **AC-010-01** _Dado_ uma instalação nova, _quando_ completo o onboarding escolhendo um modelo local, _então_ o campo de prática recebe o texto ditado e o onboarding marca sucesso.
- **AC-010-02** _Dado_ o mic bloqueado nas configurações de privacidade do Windows, _então_ o passo 2 explica e o botão abre a página correta.
- **AC-010-03** _Quando_ clico em "Tentar novamente" numa entrada que falhou, escolhendo outro provedor, _então_ uma nova transcrição é feita e a entrada é atualizada.
- **AC-010-05** _Quando_ fecho o Hub, _então_ o app continua na bandeja e os atalhos seguem funcionando.
- **AC-010-06** _Dado_ uma gravação de reunião ativa, _quando_ escolho "Sair", _então_ sou avisado e posso cancelar.
